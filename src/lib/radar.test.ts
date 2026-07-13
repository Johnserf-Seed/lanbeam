// Pure-geometry tests for the radar / trust-circle placement math. All values
// are deterministic, so we assert exact coordinates plus invariants (bounds,
// ring assignment, drag immovability).
import { describe, expect, it } from "vitest";
import {
  type ChipNode,
  hashId,
  radarBasePos,
  resolveChips,
  trustGeom,
  trustSlots,
} from "./radar";

describe("hashId", () => {
  it("is deterministic for the same id", () => {
    expect(hashId("dev-a")).toBe(hashId("dev-a"));
  });

  it("returns a 32-bit unsigned integer", () => {
    const h = hashId("some-device-id");
    expect(Number.isInteger(h)).toBe(true);
    expect(h).toBeGreaterThanOrEqual(0);
    expect(h).toBeLessThanOrEqual(0xffffffff);
  });

  it("differs for different ids", () => {
    expect(hashId("dev-a")).not.toBe(hashId("dev-b"));
  });

  it("hashes the empty string to the FNV offset basis", () => {
    expect(hashId("")).toBe(2166136261);
  });

  it("matches known reference values", () => {
    expect(hashId("dev-a")).toBe(3357282438);
    expect(hashId("dev-b")).toBe(3340504819);
    expect(hashId("phone")).toBe(2000032175);
  });
});

describe("radarBasePos", () => {
  it("produces deterministic coordinates for known inputs", () => {
    expect(radarBasePos("dev-a", 0)).toEqual({ x: 471, y: 309 });
    expect(radarBasePos("dev-b", 1)).toEqual({ x: 215, y: 327 });
    expect(radarBasePos("phone", 2)).toEqual({ x: 513, y: 183 });
    expect(radarBasePos("x", 0)).toEqual({ x: 445, y: 280 });
  });

  it("is stable across calls (no hidden state)", () => {
    expect(radarBasePos("dev-a", 0)).toEqual(radarBasePos("dev-a", 0));
  });

  it("returns integer coordinates", () => {
    const p = radarBasePos("integer-check", 4);
    expect(Number.isInteger(p.x)).toBe(true);
    expect(Number.isInteger(p.y)).toBe(true);
  });

  it("keeps points within the golden-angle ring envelope of the canvas", () => {
    // radius <= 150 + 2*42 + 18 = 252; y uses a 0.72 vertical squash.
    for (let i = 0; i < 30; i++) {
      const p = radarBasePos(`device-${i}`, i);
      const dx = p.x - 345;
      const dy = (p.y - 250) / 0.72;
      const r = Math.hypot(dx, dy);
      expect(r).toBeLessThanOrEqual(252 + 1); // +1 for rounding slack
    }
  });

  it("spreads consecutive indices apart (does not stack)", () => {
    const a = radarBasePos("same-id", 0);
    const b = radarBasePos("same-id", 1);
    expect(a).not.toEqual(b);
  });
});

describe("trustGeom", () => {
  it("uses the single-ring layout for small populations (n <= 8)", () => {
    expect(trustGeom([])).toEqual({ R: 140, rT: 0, rU: 0 });
    expect(trustGeom(Array(3).fill({ trusted: true }))).toEqual({
      R: 140,
      rT: 0,
      rU: 0,
    });
  });

  it("grows R with population but caps it at 160 by n = 8", () => {
    expect(trustGeom(Array(8).fill({ trusted: true }))).toEqual({
      R: 160,
      rT: 0,
      rU: 0,
    });
  });

  it("switches to inner/outer rings for large populations (n > 8)", () => {
    const list = Array.from({ length: 10 }, (_, i) => ({ trusted: i < 5 }));
    expect(trustGeom(list)).toEqual({ R: 142, rT: 100, rU: 182 });
  });

  it("caps rT at 130, R at 172, and rU at 190", () => {
    const list = Array.from({ length: 30 }, (_, i) => ({ trusted: i < 20 }));
    expect(trustGeom(list)).toEqual({ R: 172, rT: 130, rU: 190 });
  });
});

describe("trustSlots", () => {
  it("places a single trusted device at the top of the inner ring", () => {
    const map = trustSlots([{ id: "only", trusted: true }], 0, 0, 10, 20);
    expect(map.only).toEqual({ x: 0, y: -10 });
  });

  it("evenly distributes trusted devices and offsets untrusted ones", () => {
    const list = [
      { id: "t1", trusted: true },
      { id: "t2", trusted: true },
      { id: "u1", trusted: false },
    ];
    const map = trustSlots(list, 100, 100, 50, 80);
    expect(map.t1).toEqual({ x: 100, y: 50 });
    expect(map.t2).toEqual({ x: 100, y: 150 });
    expect(map.u1).toEqual({ x: 100, y: 180 });
  });

  it("returns a coordinate for every device", () => {
    const list = [
      { id: "a", trusted: true },
      { id: "b", trusted: false },
      { id: "c", trusted: false },
    ];
    const map = trustSlots(list, 200, 200, 60, 90);
    expect(Object.keys(map).sort()).toEqual(["a", "b", "c"]);
  });

  it("snaps to the device-pixel-ratio grid", () => {
    // dpr = 2 -> coordinates land on half-pixel boundaries.
    const map = trustSlots([{ id: "d", trusted: true }], 0, 0, 7, 10, 2);
    expect(map.d.x * 2).toBe(Math.round(map.d.x * 2));
    expect(map.d.y * 2).toBe(Math.round(map.d.y * 2));
  });

  it("places trusted devices at radius rT and untrusted at rU from center", () => {
    const list = [
      { id: "t", trusted: true },
      { id: "u", trusted: false },
    ];
    const cx = 300;
    const cy = 300;
    const map = trustSlots(list, cx, cy, 50, 90);
    expect(Math.hypot(map.t.x - cx, map.t.y - cy)).toBeCloseTo(50, 5);
    expect(Math.hypot(map.u.x - cx, map.u.y - cy)).toBeCloseTo(90, 5);
  });
});

describe("resolveChips", () => {
  const chip = (over: Partial<ChipNode> & { id: string }): ChipNode => ({
    x: 0,
    y: 0,
    mirror: false,
    inside: false,
    w: 60,
    ...over,
  });

  it("leaves non-overlapping, in-bounds chips untouched", () => {
    const a = chip({ id: "a", x: 60, y: 60, w: 40 });
    const b = chip({ id: "b", x: 300, y: 300, w: 40 });
    resolveChips([a, b], {
      tcX: 1000,
      tcY: 1000,
      R2: 50,
      bw: 600,
      bh: 600,
      drag: null,
    });
    expect(a).toEqual(chip({ id: "a", x: 60, y: 60, w: 40 }));
    expect(b).toEqual(chip({ id: "b", x: 300, y: 300, w: 40 }));
  });

  it("never moves the dragged chip", () => {
    const dragged = chip({ id: "d", x: 100, y: 100 });
    const other = chip({ id: "o", x: 110, y: 105 });
    resolveChips([dragged, other], {
      tcX: 1000,
      tcY: 1000,
      R2: 50,
      bw: 400,
      bh: 400,
      drag: "d",
    });
    expect(dragged.x).toBe(100);
    expect(dragged.y).toBe(100);
    // the non-dragged chip must have been pushed off the dragged one
    expect(other).not.toEqual(chip({ id: "o", x: 110, y: 105 }));
  });

  it("separates two overlapping chips so they no longer collide", () => {
    const a = chip({ id: "a", x: 100, y: 100 });
    const b = chip({ id: "b", x: 105, y: 102 });
    resolveChips([a, b], {
      tcX: 1000,
      tcY: 1000,
      R2: 50,
      bw: 600,
      bh: 600,
      drag: null,
    });
    // vertical separation should exceed half a row (row height 42)
    const dist = Math.hypot(a.x - b.x, a.y - b.y);
    expect(dist).toBeGreaterThan(0);
  });

  it("keeps outside chips clear of the center circle", () => {
    // a chip just inside the hub; inside=false pushes it out past R2 + 18.
    const n = chip({ id: "n", x: 310, y: 305, inside: false });
    resolveChips([n], {
      tcX: 300,
      tcY: 300,
      R2: 60,
      bw: 800,
      bh: 800,
      drag: null,
    });
    const r = Math.hypot(n.x - 300, n.y - 300);
    expect(r).toBeGreaterThanOrEqual(60 + 18 - 0.5);
  });

  it("pulls inside chips within the trust circle", () => {
    const n = chip({ id: "n", x: 900, y: 300, inside: true });
    resolveChips([n], {
      tcX: 300,
      tcY: 300,
      R2: 100,
      bw: 2000,
      bh: 800,
      drag: null,
    });
    const r = Math.hypot(n.x - 300, n.y - 300);
    expect(r).toBeLessThanOrEqual(100 - 18 + 0.5);
  });

  it("clamps chips within the vertical bounds", () => {
    const H = 42;
    const low = chip({ id: "low", x: 100, y: -500, inside: false });
    const high = chip({ id: "high", x: 200, y: 5000, inside: false });
    resolveChips([low, high], {
      tcX: 1000,
      tcY: 1000,
      R2: 10,
      bw: 600,
      bh: 400,
      drag: null,
    });
    expect(low.y).toBeGreaterThanOrEqual(H / 2 + 5);
    expect(high.y).toBeLessThanOrEqual(400 - H / 2 - 5);
  });

  it("keeps the chip's left edge inside the left boundary", () => {
    const n = chip({ id: "n", x: 0, y: 100, w: 40, inside: false });
    resolveChips([n], {
      tcX: 1000,
      tcY: 1000,
      R2: 10,
      bw: 600,
      bh: 400,
      drag: null,
    });
    // box x0 = x - anchor(16); must be >= 4
    expect(n.x - 16).toBeGreaterThanOrEqual(4 - 0.001);
  });

  it("honors custom anchor and rowH options without throwing", () => {
    const a = chip({ id: "a", x: 100, y: 100, mirror: true });
    const b = chip({ id: "b", x: 108, y: 104, mirror: true });
    expect(() =>
      resolveChips([a, b], {
        tcX: 1000,
        tcY: 1000,
        R2: 10,
        bw: 600,
        bh: 400,
        drag: null,
        anchor: 20,
        rowH: 30,
      }),
    ).not.toThrow();
  });
});
