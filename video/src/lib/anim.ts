import { Easing } from "remotion";

// Everything is timed in seconds, so the frame rate can change without
// touching a single scene.

export const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
export const lerp = (a: number, b: number, p: number) => a + (b - a) * p;

export const E = {
  /** fast start, long settle: entrances */
  out: Easing.bezier(0.16, 1, 0.3, 1),
  outSoft: Easing.bezier(0.22, 1, 0.36, 1),
  /** camera moves and morphs */
  inOut: Easing.bezier(0.65, 0, 0.35, 1),
  inOutSoft: Easing.bezier(0.45, 0, 0.25, 1),
  /** exits */
  in: Easing.bezier(0.55, 0, 0.75, 0.2),
  linear: (x: number) => x,
};

/** 0 → 1 between t0 and t1, eased; clamped outside. */
export function prog(
  t: number,
  t0: number,
  t1: number,
  ease: (x: number) => number = E.inOut,
): number {
  if (t1 <= t0) return t >= t1 ? 1 : 0;
  return ease(clamp01((t - t0) / (t1 - t0)));
}

/** A damped spring released at t0 (0 → 1, may overshoot). Analytic, so it is
 *  exact at any time, fractional frames included. */
export function spr(
  t: number,
  t0: number,
  { stiffness = 170, damping = 17, mass = 1 } = {},
): number {
  const x = t - t0;
  if (x <= 0) return 0;
  const w0 = Math.sqrt(stiffness / mass);
  const zeta = damping / (2 * Math.sqrt(stiffness * mass));
  if (zeta < 1) {
    const wd = w0 * Math.sqrt(1 - zeta * zeta);
    return (
      1 -
      Math.exp(-zeta * w0 * x) *
        (Math.cos(wd * x) + ((zeta * w0) / wd) * Math.sin(wd * x))
    );
  }
  return 1 - Math.exp(-w0 * x) * (1 + w0 * x);
}

export const SPRING = {
  pop: { stiffness: 210, damping: 15 },
  soft: { stiffness: 120, damping: 19 },
  snappy: { stiffness: 300, damping: 26 },
};

export type Pt = { x: number; y: number };

export const bez = (p0: Pt, p1: Pt, p2: Pt, p3: Pt, u: number): Pt => {
  const v = 1 - u;
  const a = v * v * v;
  const b = 3 * v * v * u;
  const c = 3 * v * u * u;
  const d = u * u * u;
  return {
    x: a * p0.x + b * p1.x + c * p2.x + d * p3.x,
    y: a * p0.y + b * p1.y + c * p2.y + d * p3.y,
  };
};

/** A flight along a cubic Bézier between t0 and t1. */
export type Flight = {
  t0: number;
  t1: number;
  p: [Pt, Pt, Pt, Pt];
  ease?: (x: number) => number;
};

/** Where a chain of flights puts the light at time t (holds at the ends). */
export function flightAt(flights: Flight[], t: number): Pt {
  let cur = flights[0];
  for (const f of flights) {
    if (t >= f.t0) cur = f;
  }
  const u = prog(t, cur.t0, cur.t1, cur.ease ?? E.inOut);
  return bez(cur.p[0], cur.p[1], cur.p[2], cur.p[3], u);
}
