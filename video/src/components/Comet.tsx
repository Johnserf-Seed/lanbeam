import { useId } from "react";
import type { Pt } from "../lib/anim";
import { C } from "../lib/theme";

/** The beam: a bright head with a tapered tail. `pts` runs head → tail (the
 *  head's recent positions), so the tail traces the real path of the light. */
export function Comet({
  pts,
  size = 9,
  opacity = 1,
  color = C.accent,
  halo = 1,
}: {
  pts: Pt[];
  size?: number;
  opacity?: number;
  color?: string;
  /** 0..1+, the soft light around the head */
  halo?: number;
}) {
  const id = useId().replace(/:/g, "");
  if (opacity <= 0.001 || pts.length === 0) return null;
  const head = pts[0];
  const tail = pts[pts.length - 1];
  const n = pts.length;

  // Tapered ribbon along the samples.
  const left: Pt[] = [];
  const right: Pt[] = [];
  let moving = false;
  for (let i = 0; i < n; i++) {
    const a = pts[Math.max(0, i - 1)];
    const b = pts[Math.min(n - 1, i + 1)];
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    const len = Math.hypot(dx, dy);
    if (len > 0.01) moving = true;
    dx = len > 0.01 ? dx / len : 0;
    dy = len > 0.01 ? dy / len : 0;
    const w = size * 0.9 * (1 - i / (n - 1)) ** 1.35;
    left.push({ x: pts[i].x - dy * w, y: pts[i].y + dx * w });
    right.push({ x: pts[i].x + dy * w, y: pts[i].y - dx * w });
  }
  const ribbon = [...left, ...right.reverse()]
    .map((p, i) => `${i ? "L" : "M"}${p.x.toFixed(1)} ${p.y.toFixed(1)}`)
    .join(" ");

  return (
    <svg
      width="100%"
      height="100%"
      style={{ position: "absolute", inset: 0, overflow: "visible", opacity }}
    >
      <defs>
        <radialGradient id={`h${id}`}>
          <stop offset="0" stopColor={color} stopOpacity={0.55} />
          <stop offset="0.35" stopColor={color} stopOpacity={0.16} />
          <stop offset="1" stopColor={color} stopOpacity={0} />
        </radialGradient>
        <radialGradient id={`c${id}`}>
          <stop offset="0" stopColor="#ffffff" />
          <stop offset="0.45" stopColor="#f2ffff" />
          <stop offset="1" stopColor={color} stopOpacity={0} />
        </radialGradient>
        <linearGradient
          id={`t${id}`}
          gradientUnits="userSpaceOnUse"
          x1={head.x}
          y1={head.y}
          x2={tail.x}
          y2={tail.y}
        >
          <stop offset="0" stopColor="#ffffff" />
          <stop offset="0.25" stopColor={color} />
          <stop offset="1" stopColor={color} stopOpacity={0} />
        </linearGradient>
        <filter id={`b${id}`} x="-50%" y="-50%" width="200%" height="200%">
          <feGaussianBlur stdDeviation={size * 0.9} />
        </filter>
      </defs>
      <circle
        cx={head.x}
        cy={head.y}
        r={size * 9 * halo}
        fill={`url(#h${id})`}
      />
      {moving && (
        <>
          <path
            d={ribbon}
            fill={color}
            opacity={0.55}
            filter={`url(#b${id})`}
          />
          <path d={ribbon} fill={`url(#t${id})`} />
        </>
      )}
      <circle cx={head.x} cy={head.y} r={size * 2.2} fill={`url(#c${id})`} />
      <circle cx={head.x} cy={head.y} r={size * 0.75} fill="#ffffff" />
    </svg>
  );
}

/** Sample a path function back in time: head first. */
export function trail(
  at: (t: number) => Pt,
  t: number,
  len = 0.22,
  n = 26,
): Pt[] {
  const out: Pt[] = [];
  for (let i = 0; i < n; i++) out.push(at(t - (len * i) / (n - 1)));
  return out;
}

/** The light carrying one scene into the next (global time). */
export function HandoffComet({
  t,
  h,
}: {
  t: number;
  h: { t0: number; t1: number; from: Pt; to: Pt; bend: number };
}) {
  if (t < h.t0 - 0.05 || t > h.t1 + 0.3) return null;
  const dx = h.to.x - h.from.x;
  const dy = h.to.y - h.from.y;
  // control points pushed sideways for an arc
  const nx = -dy * h.bend;
  const ny = dx * h.bend;
  const p1 = { x: h.from.x + dx * 0.25 + nx, y: h.from.y + dy * 0.25 + ny };
  const p2 = { x: h.from.x + dx * 0.75 + nx, y: h.from.y + dy * 0.75 + ny };
  const at = (x: number): Pt => {
    const u = Math.min(1, Math.max(0, (x - h.t0) / (h.t1 - h.t0)));
    const e = u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2;
    const v = 1 - e;
    return {
      x: v * v * v * h.from.x + 3 * v * v * e * p1.x + 3 * v * e * e * p2.x + e * e * e * h.to.x,
      y: v * v * v * h.from.y + 3 * v * v * e * p1.y + 3 * v * e * e * p2.y + e * e * e * h.to.y,
    };
  };
  const fadeIn = Math.min(1, (t - (h.t0 - 0.05)) / 0.1);
  const fadeOut = 1 - Math.min(1, Math.max(0, (t - h.t1) / 0.25));
  return <Comet pts={trail(at, t, 0.24, 28)} size={9} opacity={fadeIn * fadeOut} />;
}
