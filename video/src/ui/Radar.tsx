// The device radar (src/pages/DevicesPage.tsx), drawn around (0, 0) at the
// app's CSS sizes: rings at 110 / 190 / 270, a 58px hub, 12px device dots.
import type { CSSProperties } from "react";
import { lerp } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";

export type RadarDevice = {
  id: string;
  name: string;
  ip: string;
  x: number;
  y: number;
  /** label on the left of the dot */
  mirror?: boolean;
  /** 0..1+ (spring) */
  appear: number;
  /** "just found" pulse 0..1 (one cycle) */
  pulse?: number;
  note?: { text: string; color: string; opacity: number };
  /** drag-over highlight 0..1 */
  target?: number;
  /** transfer ring: percent 0..100, or undefined */
  ring?: number;
};

export function Radar({
  rings,
  sweep,
  sweepOpacity,
  hub,
  hubPulse,
  selfName,
  selfNameOpacity,
  devices,
  beam,
}: {
  /** ring growth 0..1, inner → outer */
  rings: [number, number, number];
  sweep: number;
  sweepOpacity: number;
  hub: number;
  /** 0..1 position in the hub's pulse cycle, or -1 */
  hubPulse: number;
  selfName: string;
  selfNameOpacity: number;
  devices: RadarDevice[];
  beam?: { to: { x: number; y: number }; draw: number; dash: number; label: string; labelOpacity: number; opacity: number };
}) {
  const abs: CSSProperties = { position: "absolute", left: 0, top: 0 };
  return (
    <div style={{ position: "relative", width: 0, height: 0, fontFamily: FONT }}>
      {/* radial wash */}
      <div
        style={{
          ...abs,
          width: 560,
          height: 560,
          margin: "-280px 0 0 -280px",
          borderRadius: "50%",
          background: `radial-gradient(circle, ${C.radial} 0%, rgba(0,0,0,0) 62%)`,
          opacity: rings[0],
        }}
      />
      {(
        [
          [270, C.ringC, rings[2]],
          [190, C.ringB, rings[1]],
          [110, C.ringA, rings[0]],
        ] as [number, string, number][]
      ).map(([r, color, g]) => {
        const rr = r * (0.55 + 0.45 * g);
        return (
          <div
            key={r}
            style={{
              ...abs,
              width: rr * 2,
              height: rr * 2,
              margin: `${-rr}px 0 0 ${-rr}px`,
              border: `1px solid ${color}`,
              borderRadius: "50%",
              opacity: Math.min(1, g * 1.3),
            }}
          />
        );
      })}
      {/* sweep: bright leading edge, fading tail behind it */}
      <div
        style={{
          ...abs,
          width: 540,
          height: 540,
          margin: "-270px 0 0 -270px",
          borderRadius: "50%",
          opacity: sweepOpacity,
          background: `conic-gradient(from ${sweep - 70}deg, rgba(0,0,0,0) 0deg, ${C.sweep} 70deg, rgba(0,0,0,0) 70.1deg)`,
        }}
      />
      {/* hub pulse */}
      {hubPulse >= 0 && (
        <div
          style={{
            ...abs,
            width: lerp(80, 168, hubPulse),
            height: lerp(80, 168, hubPulse),
            margin: `${-lerp(40, 84, hubPulse)}px 0 0 ${-lerp(40, 84, hubPulse)}px`,
            borderRadius: "50%",
            background: C.accentSoft,
            opacity: 0.55 * (1 - hubPulse) * Math.min(1, hub),
          }}
        />
      )}
      {beam && beam.opacity > 0 && <Beam {...beam} />}
      {/* hub */}
      <div
        style={{
          ...abs,
          width: 58,
          height: 58,
          margin: "-29px 0 0 -29px",
          borderRadius: "50%",
          background: C.panel,
          border: `1px solid ${C.border2}`,
          boxShadow: `0 0 24px ${C.ringA}`,
          display: "grid",
          placeItems: "center",
          fontSize: 12.5,
          fontWeight: 650,
          color: C.accentInk,
          transform: `scale(${hub})`,
          opacity: Math.min(1, hub * 2),
          zIndex: 1,
        }}
      >
        本机
      </div>
      <div
        style={{
          ...abs,
          top: 36,
          width: 300,
          marginLeft: -150,
          textAlign: "center",
          fontFamily: MONO,
          fontSize: 10.5,
          color: C.muted,
          whiteSpace: "nowrap",
          opacity: selfNameOpacity,
          zIndex: 1,
        }}
      >
        {selfName}
      </div>
      {devices.map((d) => (
        <Node key={d.id} d={d} />
      ))}
    </div>
  );
}

function Beam({
  to,
  draw,
  dash,
  label,
  labelOpacity,
  opacity,
}: {
  to: { x: number; y: number };
  draw: number;
  dash: number;
  label: string;
  labelOpacity: number;
  opacity: number;
}) {
  // From the hub's rim to the device's ring, like the app's beam.
  const len = Math.hypot(to.x, to.y);
  const ang = (Math.atan2(to.y, to.x) * 180) / Math.PI;
  const start = 31;
  const end = len - 16;
  const w = Math.max(0, (end - start) * draw);
  const mid = start + (end - start) / 2;
  const mx = (Math.cos((ang * Math.PI) / 180) * mid);
  const my = (Math.sin((ang * Math.PI) / 180) * mid);
  return (
    <>
      <div
        style={{
          position: "absolute",
          left: Math.cos((ang * Math.PI) / 180) * start,
          top: Math.sin((ang * Math.PI) / 180) * start,
          width: w,
          height: 2,
          marginTop: -1,
          transform: `rotate(${ang}deg)`,
          transformOrigin: "0 50%",
          borderRadius: 99,
          opacity,
          background: `linear-gradient(90deg, rgba(127,212,212,0.15), ${C.accent})`,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: Math.cos((ang * Math.PI) / 180) * start,
          top: Math.sin((ang * Math.PI) / 180) * start,
          width: w,
          height: 4,
          marginTop: -2,
          transform: `rotate(${ang}deg)`,
          transformOrigin: "0 50%",
          borderRadius: 99,
          opacity: 0.65 * opacity,
          background: `repeating-linear-gradient(90deg, ${C.accent} 0 6px, rgba(0,0,0,0) 6px 13px)`,
          backgroundPosition: `${dash * 26}px 0`,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: mx,
          top: my,
          width: 0,
          height: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          zIndex: 1,
          opacity: labelOpacity * opacity,
        }}
      >
        <div
          style={{
            flex: "none",
            whiteSpace: "nowrap",
            background: C.accent,
            color: C.accentFg,
            fontFamily: MONO,
            fontSize: 10,
            fontWeight: 600,
            padding: "2px 8px",
            borderRadius: 99,
            boxShadow: `0 2px 8px ${C.ringA}`,
          }}
        >
          {label}
        </div>
      </div>
    </>
  );
}

function Node({ d }: { d: RadarDevice }) {
  if (d.appear <= 0.001) return null;
  const a = Math.min(1, d.appear);
  const target = d.target ?? 0;
  const label = (
    <div
      style={{
        textAlign: d.mirror ? "right" : "left",
        opacity: a,
        transform: `translateX(${(1 - a) * (d.mirror ? 10 : -10)}px)`,
      }}
    >
      <div style={{ fontSize: 12.5, fontWeight: 600, color: C.ink2, lineHeight: "18px", whiteSpace: "nowrap" }}>
        {d.name}
      </div>
      <div style={{ fontFamily: MONO, fontSize: 10.5, color: C.muted, lineHeight: "14px", whiteSpace: "nowrap" }}>
        {d.ip}
      </div>
      {d.note && d.note.opacity > 0 && (
        <div
          style={{
            fontSize: 10.5,
            fontWeight: 600,
            color: d.note.color,
            lineHeight: "14px",
            whiteSpace: "nowrap",
            opacity: d.note.opacity,
          }}
        >
          {d.note.text}
        </div>
      )}
    </div>
  );
  const dot =
    d.ring != null ? (
      <div
        style={{
          width: 24,
          height: 24,
          borderRadius: "50%",
          background: `conic-gradient(${C.accent} ${(d.ring * 3.6).toFixed(1)}deg, ${C.track} ${(d.ring * 3.6).toFixed(1)}deg)`,
          display: "grid",
          placeItems: "center",
          flex: "none",
        }}
      >
        <div style={{ width: 17, height: 17, borderRadius: "50%", background: C.bg, display: "grid", placeItems: "center" }}>
          <div style={{ width: 9, height: 9, borderRadius: "50%", background: C.accent, boxShadow: `0 0 9px ${C.accent}` }} />
        </div>
      </div>
    ) : (
      <div style={{ width: 24, height: 24, display: "grid", placeItems: "center", flex: "none" }}>
        <div style={{ position: "relative", width: 12, height: 12, transform: `scale(${d.appear})` }}>
          {d.pulse != null && d.pulse >= 0 && d.pulse <= 1 && (
            <div
              style={{
                position: "absolute",
                inset: -lerp(7, 21, d.pulse),
                borderRadius: "50%",
                background: C.accentSoft,
                opacity: 0.9 * (1 - d.pulse),
              }}
            />
          )}
          <div
            style={{
              position: "absolute",
              inset: 0,
              borderRadius: "50%",
              background: C.accent,
              boxShadow: `0 0 12px ${C.glow}`,
            }}
          />
        </div>
      </div>
    );
  // The row is anchored so the dot's centre sits exactly on (x, y).
  return (
    <div
      style={{
        position: "absolute",
        top: d.y,
        ...(d.mirror ? { right: -d.x - 23 } : { left: d.x - 23 }),
        transform: "translate(0,-50%)",
        zIndex: 2,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: d.mirror ? "row-reverse" : "row",
          alignItems: "center",
          gap: 9,
          padding: "8px 11px",
          borderRadius: 12,
          background: target > 0 ? `rgba(127, 212, 212, ${0.12 * target})` : undefined,
          outline: `1.5px solid rgba(127, 212, 212, ${target})`,
        }}
      >
        {dot}
        {label}
      </div>
    </div>
  );
}
