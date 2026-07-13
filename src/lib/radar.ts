/** Geometry helpers shared by the device radar and the trust circle.
 *  Chip collision resolution ported verbatim from the design prototype. */

export type ChipNode = {
  id: string;
  x: number;
  y: number;
  /** chip anchors from its dot: normal chips grow rightward, mirrored leftward */
  mirror: boolean;
  /** inside the trust circle (constrains push-out) — radar passes false */
  inside: boolean;
  /** estimated rendered width */
  w: number;
};

export type ResolveOpts = {
  tcX: number;
  tcY: number;
  /** circle radius nodes must stay clear of (radar: the center hub) */
  R2: number;
  bw: number;
  bh: number;
  /** id currently being dragged — never moved by the resolver */
  drag: string | null;
  /** distance from x to the chip's leading edge (default 16) */
  anchor?: number;
  /** chip row height (default 42) */
  rowH?: number;
};

/** Iteratively push overlapping chips apart while keeping them in bounds and
 *  on their side of the circle. Mutates `list` in place. */
export function resolveChips(list: ChipNode[], opt: ResolveOpts): void {
  const gap = 6,
    H = opt.rowH || 42,
    A = opt.anchor ?? 16;
  const box = (n: ChipNode) => ({
    x0: n.mirror ? n.x + A - n.w : n.x - A,
    y0: n.y - H / 2,
  });
  for (let it = 0; it < 10; it++) {
    let moved = false;
    for (let i = 0; i < list.length; i++)
      for (let j = i + 1; j < list.length; j++) {
        const a = list[i],
          b = list[j];
        const A2 = box(a),
          B2 = box(b);
        const ox =
          Math.min(A2.x0 + a.w, B2.x0 + b.w) - Math.max(A2.x0, B2.x0) + gap;
        const oy =
          Math.min(A2.y0 + H, B2.y0 + H) - Math.max(A2.y0, B2.y0) + gap;
        if (ox <= gap || oy <= gap) continue;
        moved = true;
        if (oy <= ox) {
          const dir = a.y <= b.y ? 1 : -1;
          if (a.id === opt.drag) b.y += dir * oy;
          else if (b.id === opt.drag) a.y -= dir * oy;
          else {
            a.y -= (dir * oy) / 2;
            b.y += (dir * oy) / 2;
          }
        } else {
          const dir = a.x <= b.x ? 1 : -1;
          if (a.id === opt.drag) b.x += dir * ox;
          else if (b.id === opt.drag) a.x -= dir * ox;
          else {
            a.x -= (dir * ox) / 2;
            b.x += (dir * ox) / 2;
          }
        }
      }
    list.forEach((n) => {
      if (n.id === opt.drag) return;
      n.y = Math.min(opt.bh - H / 2 - 5, Math.max(H / 2 + 5, n.y));
      const r = Math.hypot(n.x - opt.tcX, n.y - opt.tcY) || 1;
      if (n.inside && r > opt.R2 - 18) {
        const k = (opt.R2 - 18) / r;
        n.x = opt.tcX + (n.x - opt.tcX) * k;
        n.y = opt.tcY + (n.y - opt.tcY) * k;
      }
      if (!n.inside && r < opt.R2 + 18) {
        const k = (opt.R2 + 18) / r;
        n.x = opt.tcX + (n.x - opt.tcX) * k;
        n.y = opt.tcY + (n.y - opt.tcY) * k;
      }
      const b0 = box(n);
      if (b0.x0 < 4) n.x += 4 - b0.x0;
      if (b0.x0 + n.w > opt.bw - 4) n.x -= b0.x0 + n.w - (opt.bw - 4);
    });
    if (!moved) break;
  }
}

/** Small deterministic hash for stable per-device placement. */
export function hashId(id: string): number {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** Deterministic base position (690×500 canvas, center 345/250) for a real
 *  device: golden-angle spread by discovery order, jittered by id hash so the
 *  layout is stable across sessions but devices don't stack. */
export function radarBasePos(
  id: string,
  index: number,
): { x: number; y: number } {
  const h = hashId(id);
  const angle = index * 2.39996 + ((h % 360) * Math.PI) / 180 / 6;
  const radius = 150 + ((h >> 8) % 3) * 42 + (index % 2) * 18;
  return {
    x: Math.round(345 + radius * Math.cos(angle)),
    y: Math.round(250 + radius * 0.72 * Math.sin(angle)),
  };
}

/** Slot layout for trust circle "slots"/"list" modes: trusted devices evenly
 *  on the inner radius, untrusted offset half a step on the outer radius. */
export function trustSlots(
  list: { id: string; trusted: boolean }[],
  cx: number,
  cy: number,
  rT: number,
  rU: number,
  dpr = 1,
): Record<string, { x: number; y: number }> {
  const T = list.filter((d) => d.trusted),
    U = list.filter((d) => !d.trusted);
  const snap = (v: number) => Math.round(v * dpr) / dpr;
  const map: Record<string, { x: number; y: number }> = {};
  T.forEach((d, i) => {
    const a = -Math.PI / 2 + (i * 2 * Math.PI) / Math.max(T.length, 1);
    map[d.id] = {
      x: snap(cx + rT * Math.cos(a)),
      y: snap(cy + rT * Math.sin(a)),
    };
  });
  U.forEach((d, i) => {
    const a = -Math.PI / 2 + ((i + 0.5) * 2 * Math.PI) / Math.max(U.length, 1);
    map[d.id] = {
      x: snap(cx + rU * Math.cos(a)),
      y: snap(cy + rU * Math.sin(a)),
    };
  });
  return map;
}

/** Trust circle radii by population (from the prototype). */
export function trustGeom(list: { trusted: boolean }[]): {
  R: number;
  rT: number;
  rU: number;
} {
  const n = list.length;
  const nT = list.filter((d) => d.trusted).length;
  if (n <= 8) {
    const R = Math.max(140, Math.min(160, 120 + n * 6));
    return { R, rT: 0, rU: 0 };
  }
  const rT = Math.min(130, Math.max(85, 70 + nT * 6));
  const R = Math.min(172, rT + 42);
  const rU = Math.min(190, R + 40);
  return { R, rT, rU };
}
