//! Pacing the chunk loops: the bounded pause park (M6.2) and the per-transfer
//! rate limiter (M6.7).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::consts::MAX_PAUSE_PARK;
use crate::error::{LanBeamError, Result};
use crate::state::TransferControl;

// ── cancel + pause plumbing (M6.1/6.2) ────────────────────────────────
/// Park the chunk loop while its session is paused (M6.2), waking when
/// `resume_transfer` clears the flag; a cancel while parked still ends the
/// session promptly (`Cancelled`). WHY the arm-then-recheck dance: `Notify`
/// only wakes waiters already registered when `notify_waiters` fires, so a
/// resume landing between the flag read and the await would be lost — arming
/// the waiter (`enable`) BEFORE re-reading `paused` closes that race. The
/// common "not paused" case takes a single relaxed-cost load and none of the
/// `Notify` machinery, so the per-chunk overhead is negligible.
///
/// BOUNDED (finding-4): the park is capped at [`MAX_PAUSE_PARK`] and then
/// AUTO-RESUMES. Pause is session-local with no on-wire signal, so while THIS
/// side parks the peer stays blocked in its per-chunk wire wait bounded by
/// `CHUNK_IDLE_TIMEOUT`; a park that outlived that deadline would make the peer
/// time out and abort the transfer (a >60s pause used to fail silently). By
/// resuming a safe margin before the deadline the next chunk always moves
/// inside the peer's idle window, so neither side ever times out. A keepalive
/// frame cannot lift this limit for both directions: the sender is write-only
/// mid-stream and cannot read one (and a keepalive can't cross a stalled write
/// without splitting the Noise session), so the receiver-pause direction would
/// still abort — a symmetric bound is the honest, framing-safe fix.
///
/// Returns `Ok(true)` when the cap forced an auto-resume — the caller emits
/// `transfer_resumed` so the UI clears its optimistic pause flag; `Ok(false)`
/// on the not-paused fast path or a genuine `resume_transfer`.
pub(super) async fn wait_while_paused(ctl: &TransferControl) -> Result<bool> {
    wait_while_paused_capped(ctl, MAX_PAUSE_PARK).await
}

/// The bounded park, with `cap` split out so tests exercise the auto-resume in
/// real time with a tiny cap instead of the production [`MAX_PAUSE_PARK`].
pub(super) async fn wait_while_paused_capped(ctl: &TransferControl, cap: Duration) -> Result<bool> {
    // Fast path: the overwhelmingly common case is "not paused".
    if !ctl.paused.load(Ordering::SeqCst) {
        return Ok(false);
    }
    // One deadline for the WHOLE park (spanning any spurious re-arm below), so
    // the cap bounds the total parked time, not each inner wait.
    let cap = tokio::time::sleep(cap);
    tokio::pin!(cap);
    loop {
        // Arm the waiter first so a concurrent resume (store=false → notify)
        // cannot slip through between the check below and the await.
        let notified = ctl.resume_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !ctl.paused.load(Ordering::SeqCst) {
            return Ok(false);
        }
        tokio::select! {
            biased;
            _ = ctl.cancel.cancelled() => return Err(LanBeamError::Cancelled),
            // A genuine resume is preferred over the cap when both are ready.
            _ = notified => {}
            _ = &mut cap => {
                // The cap fired first: auto-resume so the peer's idle timeout
                // never trips. Clear the flag exactly as `resume_transfer`
                // would (store=false, then wake any other waiter) and tell the
                // caller so it can surface the auto-resume to the UI.
                ctl.paused.store(false, Ordering::SeqCst);
                ctl.resume_notify.notify_waiters();
                return Ok(true);
            }
        }
    }
}

// ── rate limiting + per-file progress (M6.7/6.8) ──────────────────────
/// Per-transfer byte-rate throttle (M6.7). `None` (the `"unlimited"` setting)
/// is a true no-op — `throttle` returns on a single branch, so an unthrottled
/// transfer pays nothing. `Some(bytes_per_sec)` paces the stream with a small
/// token bucket: each chunk spends credit, and when the bucket runs dry the
/// loop sleeps just long enough to refill what it overspent. Banked credit is
/// capped at one second's worth, so a transfer that idles (paused, slow disk)
/// cannot hoard allowance and then blast past the cap on resume.
///
/// Per-transfer, NOT global (documented): two concurrent transfers each get
/// their own bucket, so the aggregate can reach N× the cap. A global limiter
/// would need shared mutable state on every chunk of every transfer; the LAN
/// case this serves (leave headroom for other traffic) is well met per-transfer.
pub(super) struct RateLimiter {
    /// Bytes per second, or `None` for unlimited.
    rate: Option<u64>,
    /// Spendable byte credit, refilled at `rate` over elapsed time.
    allowance: f64,
    /// When `allowance` was last refilled.
    last: Instant,
}

impl RateLimiter {
    /// Build a limiter from a byte-per-second cap (`None` = unlimited).
    pub(super) fn new(rate: Option<u64>) -> Self {
        Self {
            rate,
            allowance: 0.0,
            last: Instant::now(),
        }
    }

    /// Account for `n` just-transferred bytes and, if over the cap, sleep to
    /// pace the stream back down to it. A no-op when unlimited. The sleep is
    /// bounded by roughly one chunk's worth of time (chunk / rate), so it adds
    /// at most that to cancel latency — negligible even at low caps.
    pub(super) async fn throttle(&mut self, n: usize) {
        let Some(rate) = self.rate else { return };
        let rate = rate as f64;
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        // Refill, capping banked credit at one second's worth.
        self.allowance = (self.allowance + elapsed * rate).min(rate);
        self.allowance -= n as f64;
        if self.allowance < 0.0 {
            let deficit = -self.allowance;
            tokio::time::sleep(Duration::from_secs_f64(deficit / rate)).await;
            // The sleep paid the debt; start the next window from zero.
            self.allowance = 0.0;
        }
    }
}
