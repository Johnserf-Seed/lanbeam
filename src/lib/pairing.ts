/** The pairing form's state machine (PairModal), kept pure so every transition
 *  — the races included — is unit-tested without a DOM.
 *
 *  ── the model ───────────────────────────────────────────────────────────
 *  Pairing has two roles on two screens. The HOST shows its address and a
 *  6-digit code; the JOINER types both in (or pastes the pair link that packs
 *  them) and presses Join. Both then land on the same SAS compare step, and
 *  trust is recorded only when a human says the two numbers agree.
 *
 *  ── the rules that keep the two roles from colliding ────────────────────
 *  - Only the host role mints a code and only the host role's code can be
 *    redeemed. Switching tabs does NOT cancel the code — a code already read out
 *    loud must not die of a misclick.
 *  - Pressing Join cancels the host code first (the component awaits
 *    `cancel_pairing` before dialing). A device that is joining someone can
 *    therefore never also be redeemed as a host, so two people pressing Join at
 *    each other get two clean rejections, never two sessions with two SASes.
 *  - A `pair_joined` (someone redeemed OUR code) that lands while our own join
 *    is in flight is stashed, not dropped: it is the one race left (their join
 *    beat our cancel), and if our join then fails, that pairing is the live one
 *    — the other screen is already showing its SAS.
 *  - Every async result carries the `epoch` it started under and is dropped
 *    unless it still matches. Closing, re-minting and pressing Join each start
 *    a new epoch, so a late mint can't resurrect an old code and a late join
 *    can't drop a stale SAS into a fresh form. */
import type { ConnectResult, PairingInvite } from "../bridge/api";
import {
  normalizeAddr,
  normalizePairCode,
  parseJoinTarget,
  parsePairLink,
} from "./deepLink";
import { errText } from "./sendops";
import type { PairPrefill } from "./store";

export type PairRole = "host" | "join";

/** The peer the compare step shows, and which side this device was on (it
 *  picks the headline: "they accepted your invitation" vs "connected to"). */
export type ComparePeer = ConnectResult & { via: PairRole };

/** A problem shown inline, under the field it is about ("form" = below both).
 *  An i18n key + params rather than text, so the reducer stays pure and the
 *  line re-translates with the UI language. */
export type JoinError = {
  field: "addr" | "code" | "form";
  key: string;
  params?: Record<string, string>;
};

/** Where the join fields came from — the cue line under them. A link names
 *  its host (`name` "" when it didn't); an address may come with the name of
 *  the device it belongs to (the fingerprint alert) or without (a link). */
export type JoinSource =
  | { kind: "link"; name: string }
  | { kind: "addr"; name: string };

export type PairState = {
  /** True once the form has initialized for this open. The auto-mint keys on
   *  THIS, not on the overlay flag, so it never acts on a stale render. */
  open: boolean;
  /** Generation of async work; see the module comment. */
  epoch: number;
  role: PairRole;
  /* host */
  invite: PairingInvite | null;
  minting: boolean;
  mintFailed: boolean;
  /* join */
  addr: string;
  /** Digits only, ≤ 6 — the field displays it grouped. */
  code: string;
  source: JoinSource | null;
  /** A pasted link's `d=`: the join pins its handshake to that device. */
  expectedId: string | null;
  /** A join, or an add-without-pairing, in flight. */
  busy: "join" | "add" | null;
  error: JoinError | null;
  /** A `pair_joined` that landed while our own join was in flight. */
  stashed: ComparePeer | null;
  /* both */
  compare: ComparePeer | null;
};

export const initialPairState = (epoch = 0): PairState => ({
  open: false,
  epoch,
  role: "host",
  invite: null,
  minting: false,
  mintFailed: false,
  addr: "",
  code: "",
  source: null,
  expectedId: null,
  busy: null,
  error: null,
  stashed: null,
  compare: null,
});

export type PairAction =
  /** The form opened (fresh epoch); a prefill lands it on the join view. */
  | { type: "open"; epoch: number; prefill: PairPrefill | null }
  | { type: "close"; epoch: number }
  | { type: "role"; role: PairRole }
  /** A mint (first code, or 换一个) started under a fresh epoch. */
  | { type: "mintStart"; epoch: number }
  | { type: "minted"; epoch: number; invite: PairingInvite }
  | { type: "mintFailed"; epoch: number }
  /** A prefill arrived while the form was already open. */
  | { type: "prefill"; prefill: PairPrefill }
  /** The address / code field changed (a pasted pair link is unpacked). */
  | { type: "addr"; value: string }
  | { type: "code"; value: string }
  /** Forget the pasted link (its name and pin) and empty both fields. */
  | { type: "clearSource" }
  | { type: "error"; error: JoinError }
  /** Join pressed: the host code is being cancelled, a fresh epoch starts. */
  | { type: "joinStart"; epoch: number }
  | { type: "joinDone"; epoch: number; peer: ConnectResult }
  | { type: "joinFailed"; epoch: number; error: JoinError }
  /** Add-without-pairing pressed. Keeps the epoch (nothing to invalidate). */
  | { type: "addStart" }
  | { type: "addDone"; epoch: number; error?: JoinError }
  /** `pair_joined`: someone redeemed this device's code. */
  | { type: "joined"; peer: ConnectResult };

/** Fill the join fields from a prefill and show the join view. Leaves the host
 *  side alone — the code stays valid, exactly as a tab switch leaves it. */
function withPrefill(s: PairState, p: PairPrefill): PairState {
  const base: PairState = { ...s, role: "join", error: null };
  if ("link" in p) {
    const info = parsePairLink(p.link);
    if (!info)
      return { ...base, addr: "", code: "", source: null, expectedId: null };
    return {
      ...base,
      addr: info.addr ?? "",
      code: info.code,
      source: { kind: "link", name: info.name },
      expectedId: info.deviceId || null,
    };
  }
  return {
    ...base,
    addr: p.addr,
    code: "",
    source: { kind: "addr", name: p.name ?? "" },
    expectedId: null,
  };
}

/** A pair link typed or pasted into EITHER field is unpacked into both. Only a
 *  link that yields something counts, so typing `lanbeam://` by hand doesn't
 *  wipe the field halfway through. */
function unpackLink(s: PairState, value: string): PairState | null {
  const info = parsePairLink(value);
  if (!info || (!info.addr && !info.code)) return null;
  return {
    ...s,
    addr: info.addr ?? "",
    code: info.code,
    source: { kind: "link", name: info.name },
    expectedId: info.deviceId || null,
    error: null,
  };
}

export function pairReducer(s: PairState, a: PairAction): PairState {
  switch (a.type) {
    case "open": {
      const fresh: PairState = { ...initialPairState(a.epoch), open: true };
      return a.prefill ? withPrefill(fresh, a.prefill) : fresh;
    }
    case "close":
      return initialPairState(a.epoch);
    case "role":
      // A join in flight or a SAS on screen pins the view.
      if (s.busy || s.compare || s.role === a.role) return s;
      return { ...s, role: a.role, error: null };
    case "mintStart":
      if (!s.open || s.busy || s.compare) return s;
      return {
        ...s,
        epoch: a.epoch,
        invite: null,
        minting: true,
        mintFailed: false,
      };
    case "minted":
      if (a.epoch !== s.epoch || !s.minting) return s;
      return { ...s, invite: a.invite, minting: false };
    case "mintFailed":
      if (a.epoch !== s.epoch || !s.minting) return s;
      return { ...s, minting: false, mintFailed: true };
    case "prefill":
      if (!s.open || s.busy || s.compare) return s;
      return withPrefill(s, a.prefill);
    case "addr":
      if (s.busy) return s;
      return (
        unpackLink(s, a.value) ?? {
          ...s,
          addr: a.value,
          error: s.error?.field === "code" ? s.error : null,
        }
      );
    case "code":
      if (s.busy) return s;
      return (
        unpackLink(s, a.value) ?? {
          ...s,
          code: normalizePairCode(a.value),
          error: s.error?.field === "addr" ? s.error : null,
        }
      );
    case "clearSource":
      if (s.busy) return s;
      return {
        ...s,
        addr: "",
        code: "",
        source: null,
        expectedId: null,
        error: null,
      };
    case "error":
      if (s.busy) return s;
      return { ...s, error: a.error };
    case "joinStart":
      if (!s.open || s.busy || s.compare) return s;
      return {
        ...s,
        epoch: a.epoch,
        busy: "join",
        error: null,
        // The component cancels the host code before dialing; an in-flight
        // mint is invalidated by the new epoch.
        invite: null,
        minting: false,
        mintFailed: false,
        stashed: null,
      };
    case "joinDone":
      if (a.epoch !== s.epoch || s.busy !== "join") return s;
      return {
        ...s,
        busy: null,
        stashed: null,
        compare: { ...a.peer, via: "join" },
      };
    case "joinFailed":
      if (a.epoch !== s.epoch || s.busy !== "join") return s;
      // Our join failed — but someone else's join into us got through first.
      // That one is live: their screen is already showing its SAS.
      if (s.stashed)
        return { ...s, busy: null, compare: s.stashed, stashed: null };
      return { ...s, busy: null, error: a.error };
    case "addStart":
      if (!s.open || s.busy || s.compare) return s;
      return { ...s, busy: "add", error: null };
    case "addDone":
      if (a.epoch !== s.epoch || s.busy !== "add") return s;
      return { ...s, busy: null, error: a.error ?? null };
    case "joined": {
      if (!s.open || s.compare) return s;
      const peer: ComparePeer = { ...a.peer, via: "host" };
      if (s.busy === "join") return s.stashed ? s : { ...s, stashed: peer };
      return { ...s, compare: peer };
    }
  }
}

/** Check the address field. A bare 6-digit number is the classic mix-up (the
 *  code typed where the address goes) and gets its own hint. */
export function validateAddr(
  addr: string,
): { ok: true; addr: string } | { ok: false; error: JoinError } {
  const s = normalizeAddr(addr);
  if (/^\d{6}$/.test(s))
    return { ok: false, error: { field: "addr", key: "pair.codeMixup" } };
  const t = parseJoinTarget(s);
  if (!t) return { ok: false, error: { field: "addr", key: "pair.badAddr" } };
  return { ok: true, addr: `${t.ip}:${t.port}` };
}

/** Check the whole join form before anything is sent: the normalized request,
 *  or the first problem, keyed to its field. */
export function validateJoin(
  addr: string,
  code: string,
): { ok: true; addr: string; code: string } | { ok: false; error: JoinError } {
  const a = validateAddr(addr);
  if (!a.ok) return a;
  const c = normalizePairCode(code);
  if (!c)
    return {
      ok: false,
      error: { field: "code", key: "errors.pairCodeMissing" },
    };
  if (c.length < 6)
    return { ok: false, error: { field: "code", key: "pair.codeShort" } };
  return { ok: true, addr: a.addr, code: c };
}

/** Where a failed join's (or add's) reason belongs, and what it says. Keyed
 *  on the backend's stable error `kind`; the raw message is never shown.
 *  `fallback` words anything unrecognized ("pairing failed: …" for a join,
 *  "connection failed: …" for an add-without-pairing). */
export function joinErrorOf(
  e: unknown,
  addr: string,
  fallback = "pair.joinFailed",
): JoinError {
  switch ((e as { kind?: string } | null)?.kind) {
    case "InvalidAddress":
      return { field: "addr", key: "errors.invalidAddress" };
    case "PairCodeMissing":
      return { field: "code", key: "errors.pairCodeMissing" };
    case "PairRejected":
      return { field: "code", key: "pair.rejected" };
    case "Unreachable":
    case "Timeout":
      return { field: "form", key: "pair.unreachable", params: { addr } };
    case "IdentityMismatch":
      // Only a pinned join (a pasted link's `d=`) can raise this.
      return { field: "form", key: "pair.identityMismatch" };
    default:
      return { field: "form", key: fallback, params: { err: errText(e) } };
  }
}

/** `482913` → `482 913`, the way the host displays it; partial input groups
 *  the same way (`4829` → `482 9`). */
export function fmtPairCode(code: string): string {
  return code.length > 3 ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

/** The tail of a pair link that shows what it is — just the address and the
 *  code: `…a=192.168.1.20&p=51704&c=482913`. "" when it carries no address. */
export function fmtLinkPreview(link: string): string {
  const i = link.search(/[?&]a=/);
  return i < 0 ? "" : `…${link.slice(i + 1)}`;
}
