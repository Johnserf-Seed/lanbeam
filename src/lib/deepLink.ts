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
  /** Open the pairing form pre-filled from the link (the raw URL: PairModal
   *  already knows how to read `d`/`n`/`a`/`p`/`c` out of it). */
  | { cmd: "pair"; url: string }
  /** Open quick-text with the body pre-filled. */
  | { cmd: "text"; text: string }
  /** Go to Devices with the IP-direct field pre-filled. Does NOT dial. */
  | { cmd: "connect"; addr: string }
  /** Pure navigation. `open` means "just bring the window up". */
  | { cmd: "devices" | "transfers" | "inbox" | "settings" | "open" };

/** Caps on link-supplied values: an untrusted link must not be able to stuff a
 *  megabyte into the quick-text box (or a novel into the address field). */
const MAX_TEXT = 4000;
const MAX_ADDR = 64;

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
