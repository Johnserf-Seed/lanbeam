import { E, lerp, prog } from "../lib/anim";
import { C } from "../lib/theme";

export type CursorKey = { t: number; x: number; y: number; click?: boolean };

/** A macOS-style pointer that glides between keyframes and clicks. */
export function Cursor({
  t,
  keys,
  show = [0, Infinity],
  scale = 1.35,
}: {
  t: number;
  keys: CursorKey[];
  show?: [number, number];
  scale?: number;
}) {
  const vis =
    prog(t, show[0], show[0] + 0.3, E.outSoft) *
    (1 - prog(t, show[1] - 0.3, show[1], E.in));
  if (vis <= 0) return null;

  let x = keys[0].x;
  let y = keys[0].y;
  for (let i = 1; i < keys.length; i++) {
    const a = keys[i - 1];
    const b = keys[i];
    if (t >= a.t) {
      // Each leg starts after the previous key's dwell; legs take ~0.55s.
      const p = prog(t, a.t + (a.click ? 0.25 : 0), b.t, E.inOut);
      x = lerp(a.x, b.x, p);
      y = lerp(a.y, b.y, p);
    }
  }

  // Click: press on the key's time, ripple outward.
  let press = 0;
  let ripple = -1;
  for (const k of keys) {
    if (!k.click) continue;
    const d = t - k.t;
    if (d >= 0 && d < 0.6) ripple = d / 0.6;
    if (d >= 0 && d < 0.22) press = Math.sin((d / 0.22) * Math.PI);
  }

  return (
    <div
      style={{
        position: "absolute",
        left: x,
        top: y,
        width: 0,
        height: 0,
        opacity: vis,
        zIndex: 50,
      }}
    >
      {ripple >= 0 && (
        <div
          style={{
            position: "absolute",
            left: 0,
            top: 0,
            width: lerp(10, 90, E.out(ripple)),
            height: lerp(10, 90, E.out(ripple)),
            transform: "translate(-50%, -50%)",
            borderRadius: "50%",
            border: `2px solid ${C.accent}`,
            background: "rgba(127, 212, 212, 0.12)",
            opacity: 1 - ripple,
          }}
        />
      )}
      <svg
        width={24 * scale}
        height={34 * scale}
        viewBox="0 0 24 34"
        style={{
          position: "absolute",
          left: -2 * scale,
          top: -2 * scale,
          transform: `scale(${1 - press * 0.14})`,
          transformOrigin: "2px 2px",
          filter: "drop-shadow(0 4px 8px rgba(0,0,0,0.55))",
          overflow: "visible",
        }}
      >
        <path
          d="M2 2 L2 26.5 L8.2 20.6 L12.6 30.6 L16.9 28.8 L12.6 19 L20.8 19 Z"
          fill="#0b0f14"
          stroke="#ffffff"
          strokeWidth={1.7}
          strokeLinejoin="round"
        />
      </svg>
    </div>
  );
}
