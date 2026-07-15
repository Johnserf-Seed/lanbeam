/** What a dot on the trust circle means — the single source of truth.
 *
 *  Two axes, and they are NOT allowed to erase each other:
 *
 *    · COLOUR  says what you decided about this device.
 *        red   — its fingerprint changed (someone else may be answering to it)
 *        teal  — you trust it
 *        grey  — you don't
 *    · FILL    says whether it happens to be reachable this second.
 *        solid — it's here
 *        ring  — it's away
 *
 *  Presence is a passing fact. Trust is a decision you made. Letting "away"
 *  paint over the colour — which is what several hand-rolled copies of this
 *  used to do — made a trusted device look exactly like an untrusted one the
 *  moment it went to sleep, on the one page whose entire job is showing trust.
 *  Most of a trust circle is asleep at any given moment; that is the normal
 *  case, not the dead case.
 *
 *  A changed fingerprint is never hollow and never dimmed. It is the loudest
 *  thing this app has to say, and "the device is asleep" is not a reason to
 *  whisper it.
 */
import type { TrustDevice } from "./store";

export type DotTone = "alert" | "trusted" | "plain";

export type TrustDotStyle = {
  tone: DotTone;
  background: string;
  border: string;
  /** "none", or a shadow list. Compose with `withHalo` to add a selection ring. */
  boxShadow: string;
  /** Opacity for the whole node (dot + its labels), not just the dot. */
  opacity: number;
};

/** Away is quieter — but only enough to read as "not here right now". Dim far
 *  enough and it reads as "switched off", which is the thing this is guarding
 *  against: an offline device is still a device you can drag, rename and untrust. */
const AWAY_OPACITY = 0.82;

type DotInput = Pick<TrustDevice, "trusted" | "online"> & {
  fpChanged?: unknown;
};

export function trustDot(
  d: DotInput,
  opts: { ringWidth?: number } = {},
): TrustDotStyle {
  const ring = opts.ringWidth ?? 1.5;

  if (d.fpChanged) {
    return {
      tone: "alert",
      background: "var(--danger)",
      border: "none",
      boxShadow: "0 0 10px var(--danger-soft)",
      opacity: 1,
    };
  }

  if (d.trusted) {
    return d.online
      ? {
          tone: "trusted",
          background: "var(--dot-live)",
          border: "none",
          boxShadow: "0 0 10px var(--glow)",
          opacity: 1,
        }
      : {
          // Away, but still one of yours — the ring keeps the trust colour.
          tone: "trusted",
          background: "var(--accent-soft)",
          border: `${ring}px solid var(--accent)`,
          boxShadow: "none",
          opacity: AWAY_OPACITY,
        };
  }

  return d.online
    ? {
        tone: "plain",
        background: "var(--muted)",
        border: "none",
        boxShadow: "none",
        opacity: 1,
      }
    : {
        tone: "plain",
        background: "transparent",
        border: `${ring}px solid var(--muted)`,
        boxShadow: "none",
        opacity: AWAY_OPACITY,
      };
}

/** Add a selection halo outside the dot's own shadow (the halo is a spread ring,
 *  the dot's own shadow is a blur — they stack rather than replace). */
export function withHalo(shadow: string, halo: string | null): string {
  if (!halo) return shadow;
  return shadow === "none" ? halo : `${halo},${shadow}`;
}

/** Text colour matching a dot's tone. `plain` is the caller's call: a chip's
 *  sub-line wants `--muted`, a node's name wants the darker `--muted2`. */
export function toneColor(tone: DotTone, plain = "var(--muted)"): string {
  if (tone === "alert") return "var(--danger)";
  if (tone === "trusted") return "var(--accent-ink)";
  return plain;
}
