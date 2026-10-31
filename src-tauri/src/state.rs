//! Central application state, registered once via `app.manage(AppState { .. })`
//! and retrieved in commands with `state: State<AppState>`.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime};

use serde::Serialize;
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

use crate::discovery::PeerTable;
use crate::identity::Identity;
use crate::settings::Settings;
use crate::trust::TrustStore;

/// How many completed sessions [`CompletedLog`] retains (most recent first out).
pub const COMPLETED_CAP: usize = 200;

/// One recorded network degradation (UDP receive fallback, TCP ephemeral-port
/// fallback). Same shape as the `net_degraded` event payload.
///
/// WHY recorded and not just emitted: both fallbacks fire inside `setup()`,
/// before the webview has loaded and registered any listener — a Tauri emit
/// has no replay, so the UI would never see its primary scenario. The
/// frontend queries `get_net_status` once at startup instead, while the live
/// event stays for degradations after that (M4.6).
#[derive(Clone, Serialize)]
pub struct NetDegraded {
    pub kind: String,
    pub detail: String,
}

/// Incoming prompts awaiting a UI answer, by session id. See
/// `transfer::park_pending`.
pub type PendingMap = HashMap<String, Parked>;

/// One prompt waiting for the UI's answer.
pub struct Parked {
    /// Stamped at park time so the post-prompt cleanup only removes its OWN
    /// entry — a successor session that re-parked the same sender-chosen id in
    /// the interim must survive.
    pub generation: u64,
    /// Where the answer goes: a [`ReplyDecision`] (not a bare `bool`) so the
    /// ConflictModal (M6.5) can fold its collision choice into the same single
    /// ordered answer per prompt.
    pub reply: oneshot::Sender<ReplyDecision>,
    /// The `incoming_file_request` payload the prompt went out with — so a
    /// window that loaded after the event can still show it (L23).
    pub request: serde_json::Value,
}

/// How the receiver resolves a name that collides with an existing file on
/// disk (M6.5). Chosen once per transfer — from the `conflict` setting, or,
/// under the `"ask"` policy, by the user's ConflictModal reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictAction {
    /// Keep both: write the incoming file under a de-duplicated name
    /// (`report (1).pdf`). The receiver's long-standing default behavior.
    Rename,
    /// Replace the existing file with the incoming one.
    Overwrite,
}

impl ConflictAction {
    /// Parse a UI/settings string into an action; `None` for anything else
    /// (unknown value, or the `"ask"` sentinel which is resolved elsewhere) —
    /// same setters-ignore-invalid contract used across settings.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rename" => Some(Self::Rename),
            "overwrite" => Some(Self::Overwrite),
            _ => None,
        }
    }
}

/// The answer an inbound prompt is waiting for (M6.5): accept/decline plus, for
/// the `"ask"` conflict policy, how to resolve collisions. A bare `bool` no
/// longer suffices — folding the ConflictModal's choice into the same reply
/// keeps a single ordered answer per prompt (see `reply_file_request`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyDecision {
    pub accept: bool,
    /// The collision resolution the user picked, or `None` when not applicable
    /// (declined, no collision, or a caller that predates the modal). The
    /// receiver then falls back to the safe `Rename`, never overwriting unasked.
    pub conflict: Option<ConflictAction>,
}

impl ReplyDecision {
    /// A bare accept/decline with no collision choice — the shape every legacy
    /// caller and the auto-accept path produce.
    pub fn plain(accept: bool) -> Self {
        Self {
            accept,
            conflict: None,
        }
    }
}

/// Shared registry of live per-session transfer controls (M6.1/6.2), keyed by
/// session id. An entry exists for as long as a transfer can be cancelled — a
/// send registers before it dials (so a cancel while it sets up or queues is
/// honored), a receive once it is accepted — and [`TransferCtlGuard`] removes it
/// on every exit path, so cancel/pause/resume act on exactly the live sessions
/// and no-op gracefully for an id that already finished. Also what an inbound
/// session's id is checked against, so it can't borrow one of ours (L12).
/// Shared between [`AppState`] (the commands) and the listener's
/// `TransportCtx` (receive registration).
pub type TransfersCtl = Arc<Mutex<HashMap<String, TransferControl>>>;

/// The live knobs a running transfer exposes (M6.1/6.2). Cloneable because the
/// registering session and the control commands hold handles to the SAME
/// token/flag/notify — every field is a shared handle, so a clone controls the
/// same transfer.
#[derive(Clone)]
pub struct TransferControl {
    /// Tripped by `cancel_transfer`. The chunk loops `select!` on it (biased),
    /// so a cancel wins over the in-flight wire op and ends the session with
    /// [`crate::error::LanBeamError::Cancelled`]; dropping the session then
    /// makes the peer's side fail through its existing io-error path.
    pub cancel: CancellationToken,
    /// Session-local pause flag. While set, the chunk loop parks before its
    /// next chunk instead of reading/writing — TCP backpressure quietly stalls
    /// the peer, with no protocol message involved.
    pub paused: Arc<AtomicBool>,
    /// Wakes a loop parked on `paused` when `resume_transfer` clears the flag.
    pub resume_notify: Arc<Notify>,
}

impl TransferControl {
    /// A fresh control: not cancelled, not paused. Every registered session
    /// starts here; call sites that don't wire the live registry (tests, an
    /// internal caller) pass one of these as a neutral no-op.
    pub fn neutral() -> Self {
        Self {
            cancel: CancellationToken::new(),
            paused: Arc::new(AtomicBool::new(false)),
            resume_notify: Arc::new(Notify::new()),
        }
    }
}

impl Default for TransferControl {
    fn default() -> Self {
        Self::neutral()
    }
}

/// RAII registration of a session's [`TransferControl`] in a [`TransfersCtl`]
/// map. Held for the whole chunk phase; drop removes the entry so a finished,
/// cancelled or errored session leaves no stale control behind (and a later
/// cancel/pause of that id becomes a graceful no-op). Mirrors the ownership
/// model of `transfer::InFlightGuard`.
pub struct TransferCtlGuard {
    map: TransfersCtl,
    id: String,
    /// Whether THIS guard is the one that inserted `id`'s control into the map.
    /// Only the inserting guard removes on drop; a guard whose `register` found
    /// the slot already taken (or ran under a poisoned lock) holds no map entry
    /// and must leave the incumbent's alone. See [`register`].
    ///
    /// [`register`]: TransferCtlGuard::register
    owns: bool,
}

impl TransferCtlGuard {
    /// Register a fresh control for `id` and hand back both the guard and a
    /// clone of the control to drive the chunk loop. A poisoned map lock skips
    /// the insert (the transfer still runs, just uncancellable) rather than
    /// cascade the panic — same defensive contract as the pending/in-flight
    /// registries.
    ///
    /// An id already present is NOT replaced. Session ids are the sender-chosen
    /// `transfer_id`, so a peer that dials us back reusing our own outbound id
    /// would otherwise overwrite the live send's control here — redirecting its
    /// cancel/pause to the inbound session, and worse, letting whichever session
    /// drops first evict the shared entry and orphan the other's knobs. So an
    /// occupied slot leaves the incumbent in place and hands the newcomer a
    /// detached control (uncancellable, like the poisoned-lock case); its guard
    /// is marked non-owning so its drop never touches the incumbent's entry.
    pub fn register(map: &TransfersCtl, id: &str) -> (Self, TransferControl) {
        let ctl = TransferControl::neutral();
        let mut owns = false;
        if let Ok(mut m) = map.lock() {
            if !m.contains_key(id) {
                m.insert(id.to_string(), ctl.clone());
                owns = true;
            }
        }
        (
            Self {
                map: map.clone(),
                id: id.to_string(),
                owns,
            },
            ctl,
        )
    }
}

impl Drop for TransferCtlGuard {
    fn drop(&mut self) {
        // Only the guard that actually inserted this id removes it — a
        // non-owning guard (occupied slot at register, or a poisoned lock)
        // must not evict the live transfer's control.
        if !self.owns {
            return;
        }
        if let Ok(mut m) = self.map.lock() {
            m.remove(&self.id);
        }
    }
}

/// Trip the cancel token for `id` if it is registered. Returns whether an
/// entry was found — `false` means the session already finished or never
/// started, which every caller treats as a graceful no-op. A poisoned lock is
/// likewise a silent no-op.
pub fn cancel_transfer_ctl(map: &TransfersCtl, id: &str) -> bool {
    let Ok(m) = map.lock() else { return false };
    match m.get(id) {
        Some(ctl) => {
            ctl.cancel.cancel();
            true
        }
        None => false,
    }
}

/// Flip the pause flag for `id` if registered, waking the parked chunk loop on
/// resume. Returns whether an entry was found (same no-op-on-unknown contract
/// as [`cancel_transfer_ctl`]).
pub fn set_transfer_paused_ctl(map: &TransfersCtl, id: &str, paused: bool) -> bool {
    let Ok(m) = map.lock() else { return false };
    match m.get(id) {
        Some(ctl) => {
            // SeqCst so the chunk loop's pause gate can never miss a wake: the
            // store is ordered before `notify_waiters`, and the gate arms its
            // waiter before re-reading the flag (see `wait_while_paused`).
            ctl.paused.store(paused, Ordering::SeqCst);
            if !paused {
                ctl.resume_notify.notify_waiters();
            }
            true
        }
        None => false,
    }
}

/// Bounds how many transfers stream bytes at once (M6.7). Shared by both
/// directions — [`AppState`] (send) and the listener's `TransportCtx` (receive)
/// hold the SAME gate — so the cap is global across the app, not per-direction.
///
/// WHY hand-rolled instead of a `tokio::Semaphore`: the cap is the LIVE
/// `max_concurrent` setting, which the user can change at any time. A semaphore
/// fixes its permit count at construction; this gate keeps the cap in an atomic
/// that [`set_limit`] updates — and wakes the queue, so RAISING the cap admits
/// the transfers already waiting at once (M17; it used to be captured per
/// acquire, so they waited for a slot to free under the old cap). A lowered cap
/// simply gates the next transfers to reach here — ones already holding a slot
/// keep it until they finish, and the queue drains down to the new cap as they do.
///
/// A send claims its slot BEFORE dialing, so a queued send opens no socket; but
/// until the receiver answers it is only waiting on a person, and such a send
/// does not count against a RECEIVE (M11). It used to: three unanswered sends
/// held every slot for the receivers' whole 150s window, so nothing could be
/// received meanwhile — and with a cap of 1, two people sending to each other at
/// once each declined the other "busy". The cost is that a receive admitted
/// while a send waits can, once that send is accepted, briefly put one more
/// transfer on the wire than the cap: the sender can't hold an accepted
/// transfer back for long, the receiver's idle timeout is waiting on its first
/// chunk.
///
/// [`set_limit`]: ConcurrencyGate::set_limit
pub struct ConcurrencyGate {
    held: Mutex<Held>,
    /// The live cap, floored at 1.
    limit: AtomicU32,
    /// Wakes queued acquirers when a slot frees or the cap changes.
    freed: Notify,
}

/// What [`ConcurrencyGate`] counts. One lock for both numbers: a receive's
/// check reads them together, and two atomics read one after the other could
/// let a race admit a transfer the cap should refuse.
#[derive(Default)]
struct Held {
    /// Slots held: every streaming transfer, plus the sends still waiting for
    /// the receiver to answer.
    active: u32,
    /// Of `active`, the sends still waiting for that answer.
    awaiting: u32,
}

impl ConcurrencyGate {
    /// A gate admitting `limit` transfers at once — floored at 1, so a bad `0`
    /// cap can never deadlock every transfer.
    pub fn new(limit: u32) -> Self {
        Self {
            held: Mutex::new(Held::default()),
            limit: AtomicU32::new(limit.max(1)),
            freed: Notify::new(),
        }
    }

    /// Change the cap, waking every queued acquirer to re-check against it.
    pub fn set_limit(&self, limit: u32) {
        self.limit.store(limit.max(1), Ordering::Release);
        self.freed.notify_waiters();
    }

    /// Never held across an await, and nothing under it can panic; a poisoned
    /// lock still holds valid counts.
    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of slots currently held — observability for tests.
    #[doc(hidden)]
    pub fn active(&self) -> u32 {
        self.held().active
    }

    /// Claim a slot for a send without waiting, or `None` when the cap is
    /// reached (the caller then emits a "queued" nuance and awaits
    /// [`acquire_send`]). The slot counts as waiting on the receiver until
    /// [`SlotGuard::streaming`].
    ///
    /// [`acquire_send`]: ConcurrencyGate::acquire_send
    pub fn try_acquire_send(self: &Arc<Self>) -> Option<SlotGuard> {
        self.try_claim(true)
    }

    /// Claim a slot for a receive without waiting, or `None` when the cap is
    /// reached by transfers that are actually streaming.
    pub fn try_acquire_receive(self: &Arc<Self>) -> Option<SlotGuard> {
        self.try_claim(false)
    }

    /// [`try_acquire_send`](ConcurrencyGate::try_acquire_send), queuing until a slot frees.
    pub async fn acquire_send(self: &Arc<Self>) -> SlotGuard {
        self.acquire(true).await
    }

    /// [`try_acquire_receive`](ConcurrencyGate::try_acquire_receive), queuing until a slot frees.
    pub async fn acquire_receive(self: &Arc<Self>) -> SlotGuard {
        self.acquire(false).await
    }

    fn try_claim(self: &Arc<Self>, send: bool) -> Option<SlotGuard> {
        let limit = self.limit.load(Ordering::Acquire).max(1);
        let mut held = self.held();
        let counted = if send {
            held.active
        } else {
            held.active.saturating_sub(held.awaiting)
        };
        if counted >= limit {
            return None;
        }
        held.active += 1;
        if send {
            held.awaiting += 1;
        }
        Some(SlotGuard {
            gate: Arc::clone(self),
            awaiting: AtomicBool::new(send),
        })
    }

    /// WHY the arm-then-recheck dance: `Notify::notify_waiters` only wakes
    /// waiters already registered when it fires, so a slot freed between the
    /// `try_claim` below and the `await` would be lost — arming the waiter
    /// (`enable`) BEFORE trying closes that race, exactly as the transfer pause
    /// gate does.
    async fn acquire(self: &Arc<Self>, send: bool) -> SlotGuard {
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            if let Some(guard) = self.try_claim(send) {
                return guard;
            }
            freed.await;
        }
    }
}

/// RAII hold on one [`ConcurrencyGate`] slot (M6.7). Held for the whole byte
/// phase of a transfer; drop frees the slot and wakes every queued acquirer so
/// the one whose live cap now admits it can proceed.
pub struct SlotGuard {
    gate: Arc<ConcurrencyGate>,
    /// A send whose receiver has not answered yet (see [`ConcurrencyGate`]).
    awaiting: AtomicBool,
}

impl SlotGuard {
    /// The receiver accepted: from here on this send streams, and counts
    /// against receives too. Idempotent; a no-op on a receive's slot.
    pub fn streaming(&self) {
        if self.awaiting.swap(false, Ordering::AcqRel) {
            let mut held = self.gate.held();
            held.awaiting = held.awaiting.saturating_sub(1);
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        {
            let mut held = self.gate.held();
            if self.awaiting.swap(false, Ordering::AcqRel) {
                held.awaiting = held.awaiting.saturating_sub(1);
            }
            held.active = held.active.saturating_sub(1);
        }
        // Wake ALL queued acquirers (not one): a send and a receive count the
        // slots differently, so waking only one risks parking a waiter the
        // freed slot now admits behind one it still doesn't. The queue is tiny
        // (bounded by pending transfers), so the extra wakeups are negligible
        // and each waiter simply re-checks and re-parks.
        self.gate.freed.notify_waiters();
    }
}

// ── pairing (M7.1) ────────────────────────────────────────────────────
/// The single active pairing invitation minted by `start_pairing`. The joiner
/// must echo `code` back in its `PairRequest` before `expires`; the host clears
/// it on the first successful pair, on `cancel_pairing`, or when a stale one is
/// noticed. Single-active by design — `start_pairing` replaces any prior code.
pub struct PairingSession {
    /// The 6-digit code the joiner presents.
    pub code: String,
    /// Redemption deadline (`Instant`, a monotonic liveness clock — never
    /// displayed or persisted): the code is refused once `now >= expires`.
    pub expires: Instant,
    /// The same deadline on the wall clock. WHY both: `Instant` does not
    /// advance while the machine sleeps on macOS/Linux, so a code minted before
    /// a nap could stay redeemable long past its ten real minutes; the host
    /// refuses once EITHER clock says expired. Also what the UI counts down.
    pub expires_at: SystemTime,
}

/// Per-source-IP failed-`PairRequest` throttle (M7.1). Keeps a short sliding
/// window of failure timestamps per remote IP; a source over [`MAX_PAIR_FAILURES`]
/// within [`PAIR_FAILURE_WINDOW`] is refused further attempts until the window
/// rolls off, so an online brute force of the 6-digit code is cut off long
/// before it can enumerate the space. A successful pair clears the source.
///
/// [`MAX_PAIR_FAILURES`]: crate::consts::MAX_PAIR_FAILURES
/// [`PAIR_FAILURE_WINDOW`]: crate::consts::PAIR_FAILURE_WINDOW
#[derive(Default)]
pub struct PairRateLimiter {
    /// Source IP → the still-in-window failure timestamps. Absent = never failed.
    failures: HashMap<IpAddr, Vec<Instant>>,
}

impl PairRateLimiter {
    /// Whether `ip` may still attempt, WITHOUT reserving anything — a probe for
    /// tests. The pairing path always goes through [`reserve_attempt`].
    ///
    /// [`reserve_attempt`]: PairRateLimiter::reserve_attempt
    #[cfg(test)]
    pub fn allowed(&mut self, ip: IpAddr, now: Instant) -> bool {
        match self.failures.get_mut(&ip) {
            None => true,
            Some(recent) => {
                recent.retain(|t| {
                    now.saturating_duration_since(*t) < crate::consts::PAIR_FAILURE_WINDOW
                });
                (recent.len() as u32) < crate::consts::MAX_PAIR_FAILURES
            }
        }
    }

    /// Atomic check-and-RESERVE for one inbound `PairRequest` from `ip`: under a
    /// single lock hold, prune `ip`'s out-of-window failures, and if the surviving
    /// count is still under [`MAX_PAIR_FAILURES`] eagerly record this attempt
    /// (reserving its slot) and return `true`; at or over the cap, record nothing
    /// and return `false`.
    ///
    /// WHY reserve at the gate instead of checking now and recording a failure
    /// later (which is how this started): two lock acquisitions leave a window in
    /// which N concurrent requests from one IP all read the same stale count and
    /// all pass before any records its failure — a brute-force burst far past the
    /// per-window budget. Reserving under the same guard as the check makes each
    /// caller see the reservations of the ones before it, so at most
    /// [`MAX_PAIR_FAILURES`] slip through a window. The reservation is provisional:
    /// a caller that turns out to owe no real failure (a `PairRequest` at an idle
    /// host, nothing to brute-force) must undo it with [`unreserve`], and a
    /// successful pair drops it wholesale via [`clear`].
    ///
    /// [`MAX_PAIR_FAILURES`]: crate::consts::MAX_PAIR_FAILURES
    /// [`unreserve`]: PairRateLimiter::unreserve
    /// [`clear`]: PairRateLimiter::clear
    pub fn reserve_attempt(&mut self, ip: IpAddr, now: Instant) -> bool {
        let recent = self.failures.entry(ip).or_default();
        recent.retain(|t| now.saturating_duration_since(*t) < crate::consts::PAIR_FAILURE_WINDOW);
        if (recent.len() as u32) >= crate::consts::MAX_PAIR_FAILURES {
            // Over the cap: leave the surviving failures in place (non-empty, so
            // no stray entry) and refuse without consuming a slot.
            return false;
        }
        recent.push(now);
        if self.failures.len() > crate::consts::MAX_PAIR_TRACKED_IPS {
            evict_over_cap(
                &mut self.failures,
                now,
                crate::consts::MAX_PAIR_TRACKED_IPS,
                crate::consts::PAIR_FAILURE_WINDOW,
            );
        }
        true
    }

    /// Undo a reservation made by [`reserve_attempt`] when the attempt owed no
    /// real failure — a `PairRequest` that arrived while NO invitation was active
    /// has nothing to brute-force, so its slot must not linger. Removes one
    /// recorded timestamp and drops an emptied entry, preserving the property that
    /// spamming an idle host can neither grow the map nor drain a source's budget.
    ///
    /// [`reserve_attempt`]: PairRateLimiter::reserve_attempt
    pub fn unreserve(&mut self, ip: &IpAddr) {
        if let Some(recent) = self.failures.get_mut(ip) {
            recent.pop();
            if recent.is_empty() {
                self.failures.remove(ip);
            }
        }
    }

    /// Forget a source's failures after it pairs successfully.
    pub fn clear(&mut self, ip: &IpAddr) {
        self.failures.remove(ip);
    }

    /// Number of source IPs currently tracked — observability for tests (the
    /// integration tests under `tests/` build without `cfg(test)` on this crate,
    /// so this stays a plain method rather than a `#[cfg(test)]` accessor).
    #[doc(hidden)]
    pub fn tracked(&self) -> usize {
        self.failures.len()
    }
}

/// Keep a per-source-IP sliding-window map bounded to `cap` entries. First drops
/// every source whose timestamps have ALL aged out of `window` (they would be
/// pruned on their own next query but may never be queried again), then, if still
/// over the cap, evicts the least-recently-active sources (smallest most-recent
/// timestamp) until under it. Shared by the pairing and quick-text throttles.
fn evict_over_cap(
    map: &mut HashMap<IpAddr, Vec<Instant>>,
    now: Instant,
    cap: usize,
    window: std::time::Duration,
) {
    map.retain(|_, ts| {
        ts.iter()
            .any(|t| now.saturating_duration_since(*t) < window)
    });
    while map.len() > cap {
        let Some(victim) = map
            .iter()
            .min_by_key(|(_, ts)| ts.iter().max().copied())
            .map(|(ip, _)| *ip)
        else {
            break;
        };
        map.remove(&victim);
    }
}

/// Per-source-IP inbound quick-text throttle (M7.3 hardening). A short sliding
/// window of recent delivery timestamps per remote IP; a source over
/// [`MAX_TEXT_PER_WINDOW`] within [`TEXT_RATE_WINDOW`] has further texts dropped
/// until the window rolls off. Unlike the pairing throttle (which records only
/// FAILURES), EVERY delivered text counts — quick text has no accept-prompt, so
/// the rate itself is the only defense against inbox / notification spam. Bounded
/// to [`MAX_TEXT_TRACKED_IPS`] sources so a peer cycling addresses cannot grow it.
///
/// [`MAX_TEXT_PER_WINDOW`]: crate::consts::MAX_TEXT_PER_WINDOW
/// [`TEXT_RATE_WINDOW`]: crate::consts::TEXT_RATE_WINDOW
/// [`MAX_TEXT_TRACKED_IPS`]: crate::consts::MAX_TEXT_TRACKED_IPS
#[derive(Default)]
pub struct TextRateLimiter {
    /// Source IP → its still-in-window delivery timestamps. Absent = never sent.
    recent: HashMap<IpAddr, Vec<Instant>>,
}

impl TextRateLimiter {
    /// Whether a text from `ip` may be delivered now, STAMPING it when admitted.
    /// Prunes the source's out-of-window timestamps first, then admits (and
    /// records) while the in-window count is under the cap; once at the cap it
    /// drops WITHOUT stamping, so a flooding source cannot keep pushing its own
    /// window forward — it simply stays capped until the window rolls off. Evicts
    /// least-recently-active sources when the map would exceed its bound.
    pub fn admit(&mut self, ip: IpAddr, now: Instant) -> bool {
        admit_in_window(
            &mut self.recent,
            ip,
            now,
            crate::consts::MAX_TEXT_PER_WINDOW,
            crate::consts::TEXT_RATE_WINDOW,
            crate::consts::MAX_TEXT_TRACKED_IPS,
        )
    }

    /// Number of source IPs currently tracked — observability for tests.
    #[doc(hidden)]
    pub fn tracked(&self) -> usize {
        self.recent.len()
    }
}

/// Per-source-IP inbound FILE-request throttle (M18) — [`TextRateLimiter`]'s
/// sliding window, with its own budget. A file request has a prompt, but
/// reaching it costs a disk check for every listed file and, unless
/// auto-accepted, a system notification; the receive path consults this before
/// either, for any sender the user has not paired with. An over-budget request
/// is declined "busy".
#[derive(Default)]
pub struct FileRequestRateLimiter {
    recent: HashMap<IpAddr, Vec<Instant>>,
}

impl FileRequestRateLimiter {
    /// Whether a request from `ip` may proceed now, stamping it when admitted.
    pub fn admit(&mut self, ip: IpAddr, now: Instant) -> bool {
        admit_in_window(
            &mut self.recent,
            ip,
            now,
            crate::consts::MAX_FILE_REQUESTS_PER_WINDOW,
            crate::consts::FILE_REQUEST_WINDOW,
            crate::consts::MAX_FILE_REQUEST_TRACKED_IPS,
        )
    }
}

/// The sliding window behind the per-source throttles: prune `ip`'s
/// out-of-window stamps, then admit (and stamp) while the in-window count is
/// under `per_window`. Once at the cap it drops WITHOUT stamping, so a flooding
/// source cannot keep pushing its own window forward — it stays capped until the
/// window rolls off. Evicts least-recently-active sources past `max_tracked`.
fn admit_in_window(
    recent: &mut HashMap<IpAddr, Vec<Instant>>,
    ip: IpAddr,
    now: Instant,
    per_window: u32,
    window: std::time::Duration,
    max_tracked: usize,
) -> bool {
    let admitted = {
        let stamps = recent.entry(ip).or_default();
        stamps.retain(|t| now.saturating_duration_since(*t) < window);
        if (stamps.len() as u32) >= per_window {
            false
        } else {
            stamps.push(now);
            true
        }
    };
    if recent.len() > max_tracked {
        evict_over_cap(recent, now, max_tracked, window);
    }
    admitted
}

/// Shared pairing state (M7.1): the single active invitation plus the
/// per-source-IP failure throttle, behind one Arc shared by [`AppState`] (the
/// `start_pairing`/`cancel_pairing` commands mint + clear the session) and the
/// listener's `TransportCtx` (an inbound `PairRequest` matches against the
/// session and records failures in the throttle).
#[derive(Default)]
pub struct PairingState {
    pub session: Mutex<Option<PairingSession>>,
    pub rate: Mutex<PairRateLimiter>,
}

/// A peer added by hand through `connect_by_addr` (M7.2) rather than seen via
/// discovery. Kept in its own table — never expired by the discovery loop — so
/// an IP-dialed device stays on the devices page; merged BEHIND live discovery
/// entries in `list_discovered_devices`, and consulted as a fallback dial
/// target when a send/connect can't find the id in the discovery table.
#[derive(Clone)]
pub struct ManualPeer {
    pub name: String,
    pub addr: Ipv4Addr,
    pub port: u16,
    /// When this entry was last inserted or refreshed (a dial / re-pair).
    /// Monotonic [`Instant`], never displayed or persisted — the eviction key
    /// [`insert_manual_peer`] uses to keep the table bounded. Manual peers never
    /// expire (the discovery expiry loop deliberately skips them), so without an
    /// ordering carrier there would be no "oldest" to evict.
    pub last_used: Instant,
}

/// Cap on how many hand-added peers [`AppState::manual_peers`] retains within a
/// session. Manual peers never expire, so this bound is the only thing stopping
/// a long session's worth of IP dials / re-pairs — e.g. a counterpart that keeps
/// resetting its identity, minting a fresh `device_id` each time — from growing
/// the map without limit. In-memory only (cleared on restart), so this caps a
/// single session, not the whole install.
pub const MAX_MANUAL_PEERS: usize = 256;

/// Insert (or refresh) a manual peer, then evict the least-recently-used entries
/// while the table exceeds [`MAX_MANUAL_PEERS`]. Eviction is by the `last_used`
/// stamp (oldest first); re-dialing an existing id refreshes its stamp so it is
/// not the next victim. Keeps the table bounded without a dedicated expiry loop.
pub fn insert_manual_peer(
    map: &mut HashMap<String, ManualPeer>,
    device_id: String,
    peer: ManualPeer,
) {
    map.insert(device_id, peer);
    while map.len() > MAX_MANUAL_PEERS {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, p)| p.last_used)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// Completed-transfer history with bounded memory.
///
/// WHY a cap: this map lives for the whole app lifetime and every entry holds
/// the full path list of one inbound transfer. A long-running receiver (think
/// weeks of uptime on a desk machine) would otherwise grow without bound —
/// M4.6 caps it at the most recent [`COMPLETED_CAP`] sessions, which is far
/// more history than the UI's "reveal in folder" action ever reaches back for.
/// Insertion order is tracked in a side queue because `HashMap` alone cannot
/// tell us which session is oldest.
#[derive(Default)]
pub struct CompletedLog {
    map: HashMap<String, Vec<PathBuf>>,
    /// Session ids in insertion order; front = oldest = first evicted.
    order: VecDeque<String>,
}

impl CompletedLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a completed session, evicting the oldest entries beyond the cap.
    /// Re-inserting an existing id replaces its paths without changing its age.
    pub fn insert(&mut self, session_id: String, paths: Vec<PathBuf>) {
        if self.map.insert(session_id.clone(), paths).is_none() {
            self.order.push_back(session_id);
        }
        while self.order.len() > COMPLETED_CAP {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
    }

    pub fn get(&self, session_id: &str) -> Option<&Vec<PathBuf>> {
        self.map.get(session_id)
    }

    /// Number of retained sessions — observability for tests (unit tests here,
    /// plus the integration tests under `tests/`, which build without
    /// `cfg(test)` on this crate).
    #[doc(hidden)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether any session is retained — observability for tests.
    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

pub struct AppState {
    /// The device's static keypair; immutable after startup.
    pub identity: Arc<Identity>,
    /// User settings (device name, discoverable, …); mutated by commands.
    pub settings: Arc<RwLock<Settings>>,
    /// Live table of discovered LAN peers (keyed by Device ID).
    pub peers: Arc<Mutex<PeerTable>>,
    /// The TCP transfer-listener port advertised in discovery.
    pub tcp_port: Arc<AtomicU16>,
    /// Canonicalized folder that received files are written under. Behind a
    /// lock (M5.2) so `set_download_dir` retargets NEW inbound sessions
    /// without a restart; each session snapshots the path at its start and
    /// never holds the lock across an await — one session's files never
    /// split across two roots.
    pub download_dir: Arc<RwLock<PathBuf>>,
    /// Incoming transfers awaiting a UI accept/reject — see [`PendingMap`].
    pub pending: Arc<Mutex<PendingMap>>,
    /// Persistent peer trust (device_id → TrustedPeer); consulted by the
    /// receive path for policy-based auto-accept, mutated by the trust
    /// commands. Every mutation persists to `trusted.json` (M4.4).
    pub trusted: Arc<RwLock<TrustStore>>,
    /// Completed transfers: session_id → saved file paths (for "reveal"/"open").
    /// Bounded — see [`CompletedLog`].
    pub completed: Arc<Mutex<CompletedLog>>,
    /// Network degradations recorded at bind time, served by `get_net_status`
    /// — see [`NetDegraded`] for why the events alone are not enough.
    pub degraded: Arc<Mutex<Vec<NetDegraded>>>,
    /// Set once a real quit is in progress (tray 退出, `reset_identity`'s
    /// restart). WHY: with `tray_close` on, `CloseRequested` is intercepted
    /// and the window merely hides — this flag is the override that lets an
    /// intentional exit actually tear the window down (M5.3).
    pub quitting: Arc<AtomicBool>,
    /// Live controls for in-flight transfers (M6.1/6.2): a cancel token plus a
    /// pause flag/notify per session id. Shared with the listener's
    /// `TransportCtx` so both send (command side) and receive (listener side)
    /// register here and the cancel/pause/resume commands reach either. See
    /// [`TransfersCtl`].
    pub transfers_ctl: TransfersCtl,
    /// Persisted resume state for interrupted receives (M6.4): device_id →
    /// per-file `bytes_written`, backing `partials.json`. Shared with the
    /// listener's `TransportCtx` (which records/clears entries as receives
    /// interrupt/complete) so the `discard_partials` command can clear them too.
    pub partials: Arc<RwLock<crate::partials::PartialsStore>>,
    /// Global cap on simultaneously-streaming transfers (M6.7). Shared with the
    /// listener's `TransportCtx` so send (command side) and receive (listener
    /// side) draw slots from the SAME gate. See [`ConcurrencyGate`].
    pub concurrency: Arc<ConcurrencyGate>,
    /// Active pairing invitation + failure throttle (M7.1). Shared with the
    /// listener's `TransportCtx`: the `start_pairing`/`cancel_pairing` commands
    /// mint and clear the code here, and an inbound `PairRequest` matches
    /// against it (and records failures) on the listener side. See
    /// [`PairingState`].
    pub pairing: Arc<PairingState>,
    /// Peers added by hand via `connect_by_addr` (M7.2) — kept so an IP-dialed
    /// device stays visible on the devices page and reachable by a later send,
    /// independent of the discovery table's expiry. See [`ManualPeer`].
    pub manual_peers: Arc<Mutex<HashMap<String, ManualPeer>>>,
    /// Live browser shares (token → Share) for the HTTP fallback (M8.1a).
    /// Shared with the share server task so a share the share commands mint is
    /// instantly serveable. See [`crate::share::ShareRegistry`].
    pub shares: crate::share::ShareRegistry,
    /// The browser-share HTTP server's bound port (M8.1a); `0` until it binds.
    /// Read to build a share URL / advertise the port (M8.2/M8.3). See
    /// [`crate::share`].
    pub share_port: Arc<AtomicU16>,
    /// A `lanbeam://pair` link this process was LAUNCHED by (cold-start deep
    /// link), stashed because `setup()` emits the `deep_link` event before the
    /// webview can listen and Tauri events have no replay. The webview pulls it
    /// once on mount via `take_pending_deep_link`; a link that arrives while the
    /// app is already running rides the `deep_link` event instead. `None` once
    /// taken, or when the app was launched normally.
    pub pending_deep_link: Arc<Mutex<Option<String>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(i: usize) -> Vec<PathBuf> {
        vec![PathBuf::from(format!("file-{i}"))]
    }

    /// Filling past the cap must evict exactly the oldest sessions and keep
    /// the most recent `COMPLETED_CAP` reachable.
    #[test]
    fn completed_log_evicts_oldest_beyond_cap() {
        let mut log = CompletedLog::new();
        for i in 0..COMPLETED_CAP + 10 {
            log.insert(format!("s{i}"), p(i));
        }
        assert_eq!(log.len(), COMPLETED_CAP);
        // the 10 oldest are gone…
        for i in 0..10 {
            assert!(
                log.get(&format!("s{i}")).is_none(),
                "s{i} should be evicted"
            );
        }
        // …and everything newer survives, oldest-survivor and newest included.
        assert!(log.get("s10").is_some());
        assert!(log.get(&format!("s{}", COMPLETED_CAP + 9)).is_some());
    }

    /// Replacing an existing id must not grow the order queue (no double-count
    /// that would silently shrink the effective cap).
    #[test]
    fn completed_log_reinsert_replaces_without_duplicating() {
        let mut log = CompletedLog::new();
        log.insert("a".into(), p(1));
        log.insert("a".into(), p(2));
        assert_eq!(log.len(), 1);
        assert_eq!(log.get("a"), Some(&p(2)));
        // fill to the cap; "a" is oldest and must be the first evicted — once.
        for i in 0..COMPLETED_CAP {
            log.insert(format!("s{i}"), p(i));
        }
        assert!(log.get("a").is_none());
        assert_eq!(log.len(), COMPLETED_CAP);
        assert!(log.get("s0").is_some(), "only 'a' should have been evicted");
    }

    // ── transfer controls (M6.1/6.2) ──────────────────────────────────────

    /// Cancel/pause of an unknown id must be a graceful no-op (the session
    /// already finished or never started), and both must reach a registered id.
    #[test]
    fn cancel_and_pause_noop_on_unknown_but_reach_registered() {
        let map: TransfersCtl = Arc::new(Mutex::new(HashMap::new()));
        // Nothing registered yet: every knob reports "not found" and panics on
        // nothing.
        assert!(!cancel_transfer_ctl(&map, "ghost"));
        assert!(!set_transfer_paused_ctl(&map, "ghost", true));
        assert!(!set_transfer_paused_ctl(&map, "ghost", false));

        // Register one and drive it through the same helpers the commands use.
        let ctl = TransferControl::neutral();
        map.lock().unwrap().insert("live".into(), ctl.clone());
        assert!(!ctl.cancel.is_cancelled());

        assert!(set_transfer_paused_ctl(&map, "live", true));
        assert!(ctl.paused.load(Ordering::SeqCst), "pause flag flipped");
        assert!(set_transfer_paused_ctl(&map, "live", false));
        assert!(
            !ctl.paused.load(Ordering::SeqCst),
            "resume cleared the flag"
        );

        assert!(cancel_transfer_ctl(&map, "live"));
        assert!(ctl.cancel.is_cancelled(), "cancel reached the token");
    }

    /// The RAII guard registers on `register` and removes on drop, so a later
    /// cancel of the same id no-ops once the session is gone.
    #[test]
    fn ctl_guard_registers_and_removes_on_drop() {
        let map: TransfersCtl = Arc::new(Mutex::new(HashMap::new()));
        {
            let (_guard, ctl) = TransferCtlGuard::register(&map, "sess");
            assert!(
                map.lock().unwrap().contains_key("sess"),
                "registered while held"
            );
            // The returned handle drives the SAME entry that's in the map.
            assert!(cancel_transfer_ctl(&map, "sess"));
            assert!(ctl.cancel.is_cancelled());
        }
        assert!(!map.lock().unwrap().contains_key("sess"), "removed on drop");
        assert!(
            !cancel_transfer_ctl(&map, "sess"),
            "unknown after drop → no-op"
        );
    }

    // ── concurrency gate (M6.7) ───────────────────────────────────────────

    /// With a cap of 3, three transfers claim slots immediately and the 4th is
    /// refused a slot until one of the three finishes — the "3 permits + 4
    /// sessions → 4th waits" contract, proven without any network.
    #[tokio::test]
    async fn concurrency_gate_caps_at_three_and_the_fourth_waits() {
        let gate = Arc::new(ConcurrencyGate::new(3));
        // Three acquire instantly under a cap of 3.
        let g1 = gate.try_acquire_receive().expect("1st slot");
        let _g2 = gate.try_acquire_receive().expect("2nd slot");
        let g3 = gate.try_acquire_receive().expect("3rd slot");
        assert_eq!(gate.active(), 3);
        // The 4th finds no slot and would have to queue.
        assert!(
            gate.try_acquire_receive().is_none(),
            "4th must not get an immediate slot"
        );

        // A 4th acquirer parks; it must NOT resolve while the gate is full.
        let g = gate.clone();
        let fourth = tokio::spawn(async move { g.acquire_receive().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !fourth.is_finished(),
            "the 4th transfer must wait for a slot"
        );

        // Free one slot → the parked 4th proceeds, and the active count holds
        // at the cap (one left, one entered).
        drop(g1);
        let _g4 = tokio::time::timeout(std::time::Duration::from_secs(1), fourth)
            .await
            .expect("the freed slot must wake the waiter")
            .expect("acquire task must not panic");
        assert_eq!(
            gate.active(),
            3,
            "still exactly at the cap after the hand-off"
        );

        drop(g3);
        assert_eq!(gate.active(), 2);
    }

    /// Raising the cap admits a transfer that is ALREADY queued, without any
    /// slot freeing (M17) — it used to keep the cap it was called with and wait
    /// for a finish. A `0` cap is floored to 1 so it can never deadlock.
    #[tokio::test]
    async fn raising_the_cap_admits_the_queue_at_once() {
        let gate = Arc::new(ConcurrencyGate::new(1));
        let _g1 = gate.try_acquire_receive().expect("1st slot under cap 1");
        assert!(gate.try_acquire_receive().is_none(), "cap 1 is full");

        let g = gate.clone();
        let queued = tokio::spawn(async move { g.acquire_receive().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!queued.is_finished(), "parked behind the full cap");

        gate.set_limit(2);
        let _g2 = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
            .await
            .expect("the raised cap must wake the queue")
            .expect("acquire task must not panic");
        assert_eq!(gate.active(), 2);

        // A 0 cap must behave as at-least-1, never a permanent stall.
        let fresh = Arc::new(ConcurrencyGate::new(0));
        let only = fresh
            .try_acquire_receive()
            .expect("a 0 cap is floored to 1");
        drop(only);
        fresh.set_limit(0);
        assert!(fresh.try_acquire_receive().is_some(), "so is a 0 set later");
    }

    /// A send still waiting for its receiver to answer holds a slot against
    /// other SENDS (they queue before dialing, in order), but not against a
    /// RECEIVE (M11): with a cap of 1, someone sending to us while our own
    /// request waits on them used to be declined "busy" — and two people sending
    /// to each other at once declined each other. Once accepted, the send counts
    /// against everything.
    #[test]
    fn a_send_waiting_on_its_receiver_does_not_block_a_receive() {
        let gate = Arc::new(ConcurrencyGate::new(1));
        let send = gate.try_acquire_send().expect("the send's slot");
        assert!(
            gate.try_acquire_send().is_none(),
            "a second send still queues behind it"
        );
        let recv = gate
            .try_acquire_receive()
            .expect("a receive is not blocked by an unanswered send");
        drop(recv);

        send.streaming();
        send.streaming(); // idempotent
        assert!(
            gate.try_acquire_receive().is_none(),
            "an accepted send streams, and counts"
        );
        drop(send);
        assert_eq!(gate.active(), 0);

        // A send dropped while still waiting (declined, cancelled) gives back
        // its slot AND stops being counted as waiting.
        let waiting = gate.try_acquire_send().expect("slot");
        drop(waiting);
        let streaming = gate.try_acquire_receive().expect("slot");
        assert!(
            gate.try_acquire_receive().is_none(),
            "nothing left over from the dropped send"
        );
        drop(streaming);
    }

    // ── pairing throttle (M7.1) ───────────────────────────────────────────

    /// A source may attempt until it accumulates `MAX_PAIR_FAILURES` inside the
    /// window; the next attempt is refused, the window rolling off re-admits it,
    /// and a `clear` (a successful pair) resets it immediately.
    #[test]
    fn pair_rate_limiter_caps_failures_and_rolls_off() {
        use crate::consts::{MAX_PAIR_FAILURES, PAIR_FAILURE_WINDOW};
        let mut r = PairRateLimiter::default();
        let ip: IpAddr = Ipv4Addr::new(192, 168, 1, 9).into();
        let t0 = Instant::now();

        // Each attempt reserves a slot of the budget within the window.
        for i in 0..MAX_PAIR_FAILURES {
            assert!(
                r.reserve_attempt(ip, t0),
                "attempt {i} is within the budget"
            );
        }
        assert!(
            !r.reserve_attempt(ip, t0),
            "at the cap, further attempts are refused"
        );

        // Once the window rolls off, every failure ages out and the source is
        // admitted again.
        let later = t0 + PAIR_FAILURE_WINDOW + std::time::Duration::from_secs(1);
        assert!(
            r.reserve_attempt(ip, later),
            "the window rolling off re-admits the source"
        );

        // A success (clear) forgets the tally at once, even mid-window.
        while r.reserve_attempt(ip, later) {}
        r.clear(&ip);
        assert!(
            r.reserve_attempt(ip, later),
            "clear resets the source immediately"
        );

        // A different source is unaffected by another's failures.
        let other: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        while r.reserve_attempt(ip, later) {}
        assert!(r.reserve_attempt(other, later), "throttling is per-source");
    }

    /// The failure map is bounded: recording one failure each from far more
    /// distinct source IPs than the cap (the "cycle through addresses" leak)
    /// leaves the map at or under [`MAX_PAIR_TRACKED_IPS`], never unbounded.
    #[test]
    fn pair_rate_limiter_map_stays_bounded() {
        use crate::consts::MAX_PAIR_TRACKED_IPS;
        let mut r = PairRateLimiter::default();
        let now = Instant::now();
        // Each IP fails exactly once and never returns — nothing would prune it.
        for i in 0..(MAX_PAIR_TRACKED_IPS as u32 + 500) {
            let ip: IpAddr = Ipv4Addr::from(i.wrapping_add(1)).into();
            r.reserve_attempt(ip, now);
        }
        assert!(
            r.tracked() <= MAX_PAIR_TRACKED_IPS,
            "map bounded to the cap, got {}",
            r.tracked()
        );
    }

    /// The concurrent-burst defense: N `PairRequest`s from ONE IP racing through
    /// the reserve-on-check gate must let AT MOST `MAX_PAIR_FAILURES` slip inside
    /// a single window. Each caller reserves its slot under the shared lock, so
    /// the next one sees it — the pre-fix TOCTOU (every task reading a stale
    /// count 0 before any recorded a failure) can no longer hand out more than the
    /// per-window budget.
    #[test]
    fn pair_rate_limiter_concurrent_reservations_capped_to_budget() {
        use crate::consts::MAX_PAIR_FAILURES;
        let limiter = Arc::new(Mutex::new(PairRateLimiter::default()));
        let ip: IpAddr = Ipv4Addr::new(203, 0, 113, 7).into();
        // One shared instant: nothing ages out mid-burst, so the cap is the only
        // thing that can gate the reservations.
        let now = Instant::now();
        let admitted = Arc::new(AtomicU32::new(0));

        let mut handles = Vec::new();
        for _ in 0..200u32 {
            let limiter = Arc::clone(&limiter);
            let admitted = Arc::clone(&admitted);
            handles.push(std::thread::spawn(move || {
                if limiter.lock().unwrap().reserve_attempt(ip, now) {
                    admitted.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            admitted.load(Ordering::SeqCst),
            MAX_PAIR_FAILURES,
            "a concurrent burst from one IP admits at most the per-window budget"
        );
        // The reservations left behind are exactly the budget's worth of real
        // failures, so the source is now throttled.
        assert!(!limiter.lock().unwrap().allowed(ip, now));
    }

    /// A reservation for a request that owed no real failure (a `PairRequest` at
    /// an idle host) is rolled back: `unreserve` neither grows the map nor drains
    /// the budget, and drops the emptied entry so an idle-host flood leaves no
    /// residue — the deliberate anti-amplification property.
    #[test]
    fn pair_rate_limiter_unreserve_rolls_back_idle_host_attempts() {
        use crate::consts::MAX_PAIR_FAILURES;
        let mut r = PairRateLimiter::default();
        let ip: IpAddr = Ipv4Addr::new(198, 51, 100, 4).into();
        let now = Instant::now();

        // Many idle-host attempts: each reserves, then rolls back. Net residue is
        // nil, so the budget is never consumed however long the flood runs.
        for _ in 0..(MAX_PAIR_FAILURES * 10) {
            assert!(
                r.reserve_attempt(ip, now),
                "an unspent budget always admits"
            );
            r.unreserve(&ip);
        }
        assert_eq!(r.tracked(), 0, "idle-host flood leaves no tracked source");
        assert!(r.allowed(ip, now), "budget untouched after rollbacks");

        // A reservation LEFT in place (a real miss against a live code) counts as
        // a failure — unreserve only undoes a just-added slot, not real ones.
        for _ in 0..MAX_PAIR_FAILURES {
            assert!(r.reserve_attempt(ip, now));
        }
        assert!(!r.allowed(ip, now), "kept reservations exhaust the budget");
    }

    // ── quick-text throttle (M7.3 hardening) ──────────────────────────────

    /// A source may deliver up to [`MAX_TEXT_PER_WINDOW`] texts inside the
    /// window; the next is dropped, and the window rolling off re-admits it.
    #[test]
    fn text_rate_limiter_caps_and_rolls_off() {
        use crate::consts::{MAX_TEXT_PER_WINDOW, TEXT_RATE_WINDOW};
        let mut r = TextRateLimiter::default();
        let ip: IpAddr = Ipv4Addr::new(192, 168, 1, 42).into();
        let t0 = Instant::now();

        for i in 0..MAX_TEXT_PER_WINDOW {
            assert!(r.admit(ip, t0), "text {i} within the budget is admitted");
        }
        assert!(!r.admit(ip, t0), "the text over the budget is dropped");

        // A dropped text must NOT push the window forward: still capped a moment
        // later, before anything ages out.
        assert!(!r.admit(ip, t0), "a flooding source stays capped");

        // Once the window rolls off, the source is admitted again.
        let later = t0 + TEXT_RATE_WINDOW + std::time::Duration::from_secs(1);
        assert!(
            r.admit(ip, later),
            "the window rolling off re-admits the source"
        );

        // Per-source: a different IP has its own budget.
        let other: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();
        assert!(r.admit(other, later), "throttling is per-source");
    }

    /// The file-request throttle (M18) has its own budget on the same sliding
    /// window: the request past it is refused, refusals don't extend the
    /// window, and it rolls off.
    #[test]
    fn file_request_limiter_caps_and_rolls_off() {
        use crate::consts::{FILE_REQUEST_WINDOW, MAX_FILE_REQUESTS_PER_WINDOW};
        let mut r = FileRequestRateLimiter::default();
        let ip: IpAddr = Ipv4Addr::new(192, 168, 1, 42).into();
        let t0 = Instant::now();
        for i in 0..MAX_FILE_REQUESTS_PER_WINDOW {
            assert!(r.admit(ip, t0), "request {i} within the budget proceeds");
        }
        assert!(!r.admit(ip, t0), "the request over the budget is refused");
        assert!(!r.admit(ip, t0), "and stays refused");
        let other: IpAddr = Ipv4Addr::new(10, 0, 0, 2).into();
        assert!(r.admit(other, t0), "per source");
        let later = t0 + FILE_REQUEST_WINDOW + std::time::Duration::from_secs(1);
        assert!(r.admit(ip, later), "the window rolling off re-admits it");
    }

    /// The quick-text map is bounded the same way the pairing map is.
    #[test]
    fn text_rate_limiter_map_stays_bounded() {
        use crate::consts::MAX_TEXT_TRACKED_IPS;
        let mut r = TextRateLimiter::default();
        let now = Instant::now();
        for i in 0..(MAX_TEXT_TRACKED_IPS as u32 + 500) {
            let ip: IpAddr = Ipv4Addr::from(i.wrapping_add(1)).into();
            r.admit(ip, now);
        }
        assert!(
            r.tracked() <= MAX_TEXT_TRACKED_IPS,
            "map bounded to the cap, got {}",
            r.tracked()
        );
    }

    // ── conflict action parsing (M6.5) ────────────────────────────────────

    /// Only the two real actions parse; the `"ask"` sentinel and any unknown
    /// value are rejected (`None`), matching the setters-ignore-invalid contract.
    #[test]
    fn conflict_action_parse_recognizes_known_and_rejects_rest() {
        assert_eq!(
            ConflictAction::parse("rename"),
            Some(ConflictAction::Rename)
        );
        assert_eq!(
            ConflictAction::parse("overwrite"),
            Some(ConflictAction::Overwrite)
        );
        // The "ask" policy is resolved elsewhere, so it is not a parseable action.
        assert_eq!(ConflictAction::parse("ask"), None);
        assert_eq!(ConflictAction::parse(""), None);
        assert_eq!(ConflictAction::parse("Rename"), None, "case-sensitive");
    }

    // ── transfer control defaults / guard identity (M6.1/6.2) ──────────────

    /// The `Default` impl mirrors `neutral()`: not cancelled, not paused.
    #[test]
    fn transfer_control_default_is_neutral() {
        let ctl = TransferControl::default();
        assert!(!ctl.cancel.is_cancelled());
        assert!(!ctl.paused.load(Ordering::SeqCst));
    }

    /// A second `register` of an id already present must NOT replace the
    /// incumbent's control (the finding-hardened identity path): the newcomer
    /// gets a detached control and a non-owning guard, so its drop leaves the
    /// live transfer's entry untouched while the owning guard's drop removes it.
    #[test]
    fn ctl_guard_second_register_of_same_id_is_non_owning() {
        let map: TransfersCtl = Arc::new(Mutex::new(HashMap::new()));
        let (guard1, ctl1) = TransferCtlGuard::register(&map, "dup");
        let (guard2, ctl2) = TransferCtlGuard::register(&map, "dup");

        // The map still holds the incumbent — cancel via the map reaches ctl1,
        // never the newcomer's detached control.
        assert!(cancel_transfer_ctl(&map, "dup"));
        assert!(ctl1.cancel.is_cancelled(), "map entry is the incumbent's");
        assert!(
            !ctl2.cancel.is_cancelled(),
            "newcomer got a detached, uncancellable control"
        );

        // Dropping the non-owning newcomer must spare the incumbent's entry.
        drop(guard2);
        assert!(
            map.lock().unwrap().contains_key("dup"),
            "non-owning drop leaves the incumbent in place"
        );

        // Only the owning guard removes it.
        drop(guard1);
        assert!(
            !map.lock().unwrap().contains_key("dup"),
            "owning drop removes the entry"
        );
    }

    // ── pairing state (M7.1) ──────────────────────────────────────────────

    /// The shared state exposes an initially-empty session and a usable throttle
    /// behind the same handle: mint an invitation, read it back, and gate a source.
    #[test]
    fn pairing_state_holds_session_and_rate() {
        let ps = PairingState::default();
        assert!(ps.session.lock().unwrap().is_none(), "no session at first");

        *ps.session.lock().unwrap() = Some(PairingSession {
            code: "123456".into(),
            expires: Instant::now() + std::time::Duration::from_secs(60),
            expires_at: SystemTime::now() + std::time::Duration::from_secs(60),
        });
        assert_eq!(
            ps.session.lock().unwrap().as_ref().unwrap().code,
            "123456",
            "the minted code reads back"
        );

        // The throttle behind the same state admits a fresh source.
        let ip: IpAddr = Ipv4Addr::new(172, 16, 0, 5).into();
        assert!(ps.rate.lock().unwrap().allowed(ip, Instant::now()));
    }

    // ── manual peer table (M7.2) ──────────────────────────────────────────

    /// Filling past the cap evicts the least-recently-used entry (smallest
    /// `last_used`), keeping the table bounded to [`MAX_MANUAL_PEERS`].
    #[test]
    fn insert_manual_peer_evicts_oldest_beyond_cap() {
        let mut map: HashMap<String, ManualPeer> = HashMap::new();
        let t0 = Instant::now();
        let mk = |i: u64| ManualPeer {
            name: format!("dev-{i}"),
            addr: Ipv4Addr::new(192, 168, 0, 1),
            port: 5000,
            last_used: t0 + std::time::Duration::from_millis(i),
        };
        for i in 0..(MAX_MANUAL_PEERS as u64) {
            insert_manual_peer(&mut map, format!("id-{i}"), mk(i));
        }
        assert_eq!(map.len(), MAX_MANUAL_PEERS);

        // One more with the newest stamp pushes over the cap: the oldest (id-0) goes.
        insert_manual_peer(&mut map, "id-new".into(), mk(MAX_MANUAL_PEERS as u64 + 100));
        assert_eq!(map.len(), MAX_MANUAL_PEERS, "still bounded after eviction");
        assert!(!map.contains_key("id-0"), "least-recently-used evicted");
        assert!(map.contains_key("id-1"), "next-oldest survives");
        assert!(map.contains_key("id-new"), "the new peer is retained");
    }

    /// Re-inserting an existing id refreshes it in place (new fields, same slot)
    /// without growing the table.
    #[test]
    fn insert_manual_peer_refresh_updates_without_growing() {
        let mut map: HashMap<String, ManualPeer> = HashMap::new();
        let t0 = Instant::now();
        insert_manual_peer(
            &mut map,
            "a".into(),
            ManualPeer {
                name: "a".into(),
                addr: Ipv4Addr::LOCALHOST,
                port: 1,
                last_used: t0,
            },
        );
        insert_manual_peer(
            &mut map,
            "a".into(),
            ManualPeer {
                name: "a2".into(),
                addr: Ipv4Addr::LOCALHOST,
                port: 2,
                last_used: t0 + std::time::Duration::from_secs(1),
            },
        );
        assert_eq!(map.len(), 1, "refresh replaces, does not add");
        assert_eq!(map.get("a").unwrap().name, "a2");
        assert_eq!(map.get("a").unwrap().port, 2);
    }

    /// Re-dialing the oldest entry refreshes its `last_used` stamp, so the NEXT
    /// eviction takes a different (now-oldest) victim rather than the refreshed one.
    #[test]
    fn insert_manual_peer_refresh_spares_from_eviction() {
        let mut map: HashMap<String, ManualPeer> = HashMap::new();
        let t0 = Instant::now();
        let mk = |i: u64, t: Instant| ManualPeer {
            name: format!("d{i}"),
            addr: Ipv4Addr::LOCALHOST,
            port: 1,
            last_used: t,
        };
        for i in 0..(MAX_MANUAL_PEERS as u64) {
            insert_manual_peer(
                &mut map,
                format!("id-{i}"),
                mk(i, t0 + std::time::Duration::from_millis(i)),
            );
        }
        // Refresh the oldest (id-0) with a fresh stamp so it is no longer the victim.
        insert_manual_peer(
            &mut map,
            "id-0".into(),
            mk(0, t0 + std::time::Duration::from_secs(3600)),
        );
        // A new peer pushes over the cap: now id-1 (next-oldest) is evicted, not id-0.
        insert_manual_peer(
            &mut map,
            "id-new".into(),
            mk(999, t0 + std::time::Duration::from_secs(7200)),
        );
        assert_eq!(map.len(), MAX_MANUAL_PEERS);
        assert!(map.contains_key("id-0"), "refreshed peer spared");
        assert!(!map.contains_key("id-1"), "next-oldest evicted instead");
    }
}
