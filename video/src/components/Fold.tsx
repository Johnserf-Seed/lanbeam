import type { ReactNode } from "react";
import { E, lerp, prog } from "../lib/anim";
import { C } from "../lib/theme";

/** Fold a panel into a point of light (at `fold`), or open one out of it (at
 *  `open`): first flat into a bright line, then down to a dot — and back. */
export function foldState(t: number, open: number, fold = Infinity) {
  const ox = prog(t, open, open + 0.26, E.out);
  const oy = prog(t, open + 0.16, open + 0.55, E.out);
  const fy = prog(t, fold, fold + 0.26, E.in);
  const fx = prog(t, fold + 0.2, fold + 0.46, E.inOut);
  const sx = lerp(0.003, 1, ox) * lerp(1, 0.003, fx);
  const sy = lerp(0.008, 1, oy) * lerp(1, 0.008, fy);
  const line = Math.max(ox * (1 - oy), fy * (1 - fx));
  const visible = ox > 0 && fx < 1;
  return { sx, sy, line, visible };
}

export function Fold({
  x,
  y,
  t,
  open,
  fold,
  width,
  children,
}: {
  x: number;
  y: number;
  t: number;
  open: number;
  fold?: number;
  /** on-screen width of the panel, for the light line */
  width: number;
  children: ReactNode;
}) {
  const s = foldState(t, open, fold);
  if (!s.visible) return null;
  const drift = 1 + 0.022 * prog(t, open, fold !== undefined && Number.isFinite(fold) ? fold : open + 6, (x) => x);
  return (
    <div style={{ position: "absolute", left: x, top: y, width: 0, height: 0 }}>
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          transform: `translate(-50%, -50%) scale(${s.sx * drift}, ${s.sy * drift})`,
          filter: s.line > 0.02 ? `brightness(${1 + s.line * 2.5})` : undefined,
        }}
      >
        {children}
      </div>
      {s.line > 0.01 && (
        <div
          style={{
            position: "absolute",
            left: -(width * s.sx) / 2 - 20,
            top: -2,
            width: width * s.sx + 40,
            height: 4,
            borderRadius: 4,
            background: `linear-gradient(90deg, rgba(127,212,212,0) 0%, ${C.accent} 18%, #ffffff 50%, ${C.accent} 82%, rgba(127,212,212,0) 100%)`,
            boxShadow: `0 0 24px 6px rgba(127,212,212,${0.55 * s.line})`,
            opacity: s.line,
          }}
        />
      )}
    </div>
  );
}
