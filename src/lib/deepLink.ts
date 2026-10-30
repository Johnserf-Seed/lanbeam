/** The `lanbeam://` command surface.
 *
 *  ── SECURITY CONTRACT — read this before adding anything ──────────────────
 *  A deep link is UNTRUSTED input: any web page can ask the OS to open one, and
 *  the user may not even realise a link was followed. So every command here is
 *  confined to exactly three verbs — SURFACE, PRE-FILL, NAVIGATE.
 *
 *  Not one of them may pair, connect, send, trust, share, or change a setting.
 *  `connect` pre-fills the IP field and stops; `text` pre-fills the box and the
 *  user still picks a device and presses send; `pair` pre-fills the form and the
 *  user still confirms the SAS.
 *
 *  **A link that decides something for the user is a link an ATTACKER gets to
 *  decide with.** Adding a command that *acts* is the one change that must never
 *  be made here, however convenient it sounds.
 *  ─────────────────────────────────────────────────────────────────────────── */

/** Everything a link may carry. An unknown command is dropped — never guessed. */
export type DeepLink =
  /** Open the pairing form's join view pre-filled from the link (the raw URL:
   *  `parsePairLink` reads `d`/`n`/`a`/`p`/`c` out of it). */
  | { cmd: "pair"; url: string }
  /** Open quick-text with the body pre-filled. */
  | { cmd: "text"; text: string }
  /** Open the pairing form's join view with the address pre-filled. Does NOT
   *  dial. */
  | { cmd: "connect"; addr: string }
  /** Pure navigation. `open` means "just bring the window up". */
  | { cmd: "devices" | "transfers" | "inbox" | "settings" | "open" };

/** Caps on link-supplied values: an untrusted link must not be able to stuff a
 *  megabyte into the quick-text box (or a novel into the address field). */
const MAX_TEXT = 4000;
const MAX_ADDR = 64;
/** A real pair link is ~200 bytes (a 63-char CJK name percent-encodes to ~570
 *  of them at the very worst — `n=` is the only long field). Anything past this
 *  is not an invitation we minted. */
export const MAX_PAIR_LINK = 1024;

/** LanBeam's default TCP listen port — what a bare `ip` (no `:port`) dials,
 *  exactly as the backend's `parse_dial_target` defaults it. */
export const DEFAULT_TCP_PORT = 51704;

/** Parse a `lanbeam://<cmd>[?params]` link. `null` for anything that is not one
 *  of ours, or whose required parameter is missing — we never guess an intent. */
export function parseDeepLink(raw: string): DeepLink | null {
  const s = raw.trim();
  if (!s.toLowerCase().startsWith("lanbeam://")) return null;

  let u: URL;
  try {
    u = new URL(s);
  } catch {
    return null;
  }
  // `lanbeam://text?t=hi` parses with host "text" (a non-special scheme still
  // takes an authority after "//"). Fall back to the path for odd shapes.
  const cmd = (u.host || u.pathname.replace(/^\/+/, "")).toLowerCase();

  switch (cmd) {
    case "pair":
      if (s.length > MAX_PAIR_LINK) return null;
      return { cmd: "pair", url: s };

    case "text": {
      const t = u.searchParams.get("t") ?? "";
      if (!t) return null;
      return { cmd: "text", text: t.slice(0, MAX_TEXT) };
    }

    case "connect": {
      const a = (u.searchParams.get("a") ?? "").trim();
      // Only a length guard here: the field is a PRE-FILL the user still has to
      // act on, and the real address validation lives where the dial happens.
      if (!a || a.length > MAX_ADDR) return null;
      return { cmd: "connect", addr: a };
    }

    case "devices":
    case "transfers":
    case "inbox":
    case "settings":
    case "open":
      return { cmd };

    default:
      return null;
  }
}

/* ── pair links + join targets ───────────────────────────────────────────
   A pair link is just an address and a pairing code (plus the host's name and
   Device ID) packed into one string. The pairing form unpacks it into the two
   fields it shows, so what the user sees is exactly what gets sent. */

/** What a `lanbeam://pair` link carries, unpacked for the join form. */
export type PairLinkInfo = {
  /** `ip:port`, validated; null when the link has no usable `a=`. */
  addr: string | null;
  /** The `c=` code, normalized to digits (may be ""). */
  code: string;
  /** The host's display name (`n=`), trimmed and capped; "" when absent. */
  name: string;
  /** The host's Device ID (`d=`) when it is well-formed, else "". The join
   *  pins its handshake to it. */
  deviceId: string;
};

/** A Device ID is a base64url (unpadded) 32-byte X25519 key: 43 characters. */
const DEVICE_ID_RE = /^[A-Za-z0-9_-]{43}$/;

/** Unpack a `lanbeam://pair?…` link. null for anything else — another
 *  `lanbeam://` command, a different scheme, junk, or an oversized string. A
 *  malformed `p=` falls back to the default port and a malformed `d=` is simply
 *  no pin, mirroring the backend's `parse_pair_uri`. */
export function parsePairLink(raw: string): PairLinkInfo | null {
  const s = raw.trim();
  if (s.length > MAX_PAIR_LINK || !/^lanbeam:\/\//i.test(s)) return null;
  let u: URL;
  try {
    u = new URL(s);
  } catch {
    return null;
  }
  const cmd = (u.host || u.pathname.replace(/^\/+/, ""))
    .replace(/\/+$/, "")
    .toLowerCase();
  if (cmd !== "pair") return null;
  const q = u.searchParams;
  const a = (q.get("a") ?? "").trim();
  const p = Number((q.get("p") ?? "").trim());
  const port = Number.isInteger(p) && p > 0 && p <= 65535 ? p : null;
  const target = a ? parseJoinTarget(port ? `${a}:${port}` : a) : null;
  const d = (q.get("d") ?? "").trim();
  return {
    addr: target ? `${target.ip}:${target.port}` : null,
    code: normalizePairCode(q.get("c") ?? ""),
    // By character, not UTF-16 unit: a cut must never split an emoji.
    name: Array.from((q.get("n") ?? "").trim())
      .slice(0, 64)
      .join(""),
    deviceId: DEVICE_ID_RE.test(d) ? d : "",
  };
}

/** A pairing code reduced to what it is: up to six ASCII digits. Drops the
 *  grouping space the host DISPLAYS ("482 913") and any dash, and folds
 *  full-width digits (`４８２`, from a CJK input method) onto ASCII — the
 *  backend's `normalize_pair_code` does the same on both ends. */
export function normalizePairCode(raw: string): string {
  let out = "";
  for (const ch of raw) {
    const c = ch.charCodeAt(0);
    if (c >= 0x30 && c <= 0x39) out += ch;
    else if (c >= 0xff10 && c <= 0xff19)
      out += String.fromCharCode(c - 0xff10 + 0x30);
    if (out.length === 6) break;
  }
  return out;
}

/** Tidy a typed address the way a person types one: drop every space (the
 *  host shows `192.168.1.20:51704`, but people copy by eye), and fold the
 *  full-width digits / colon / full stop a CJK input method produces. */
export function normalizeAddr(raw: string): string {
  let out = "";
  for (const ch of raw) {
    const c = ch.charCodeAt(0);
    if (/\s/.test(ch)) continue;
    if (c >= 0xff10 && c <= 0xff19)
      out += String.fromCharCode(c - 0xff10 + 0x30);
    else if (ch === "：") out += ":";
    else if (ch === "。" || ch === "．" || ch === "｡") out += ".";
    else out += ch;
  }
  return out;
}

/** Parse what the join form's address field holds: `ip` or `ip:port`, IPv4
 *  only (all the backend dials). Octets with leading zeros are refused, as
 *  Rust's `Ipv4Addr` parser refuses them, and so is port 0. null = not an
 *  address — the caller says so next to the field instead of sending it. */
export function parseJoinTarget(
  raw: string,
): { ip: string; port: number } | null {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?::(\d{1,5}))?$/.exec(
    normalizeAddr(raw),
  );
  if (!m) return null;
  const octets = m.slice(1, 5);
  if (octets.some((o) => Number(o) > 255 || (o.length > 1 && o[0] === "0")))
    return null;
  const port = m[5] === undefined ? DEFAULT_TCP_PORT : Number(m[5]);
  if (port < 1 || port > 65535) return null;
  return { ip: octets.join("."), port };
}
