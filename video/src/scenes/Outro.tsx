import { AbsoluteFill } from "remotion";
import { BrandMark } from "../components/BrandMark";
import { CharReveal, reveal } from "../components/Text";
import { E, lerp, prog, spr, SPRING } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";
import { H, T } from "../lib/timeline";

const S = 200;
const MX = 660; // mark centre; its dot is where the light lands (H.lookOutro.to)
const MY = 420;

export function Outro({ t }: { t: number }) {
  const o = t;
  const land = H.lookOutro.t1 - T.outro[0];
  const end = T.outro[1] - T.outro[0];

  const flash = prog(o, land - 0.02, land + 0.7, E.out);
  const dot = o < land ? 0 : spr(o, land - 0.04, SPRING.pop);
  const square = spr(o, land + 0.15, SPRING.soft);
  const arcs = [0, 1, 2].map((i) => prog(o, land + 0.08 + i * 0.11, land + 0.5 + i * 0.11, E.out)) as [number, number, number];
  const fade = prog(o, end - 0.9, end - 0.05, E.inOut);

  const pills = ["不上云", "不注册", "不中转"];

  return (
    <AbsoluteFill style={{ opacity: 1 - fade }}>
      {flash > 0 && flash < 1 && (
        <div
          style={{
            position: "absolute",
            left: H.lookOutro.to.x,
            top: H.lookOutro.to.y,
            width: lerp(20, 900, flash),
            height: lerp(20, 900, flash),
            transform: "translate(-50%, -50%)",
            borderRadius: "50%",
            background: "radial-gradient(circle, rgba(196,250,250,0.5) 0%, rgba(127,212,212,0.16) 35%, rgba(127,212,212,0) 70%)",
            opacity: 1 - flash,
          }}
        />
      )}
      <div style={{ position: "absolute", left: MX - S / 2, top: MY - S / 2 }}>
        <BrandMark size={S} square={square} dot={dot} arcs={arcs} />
      </div>
      <div style={{ position: "absolute", left: MX + S / 2 + 44, top: MY - 74, fontFamily: FONT }}>
        <div
          style={{
            fontSize: 132,
            fontWeight: 600,
            letterSpacing: "-0.02em",
            lineHeight: 1,
            color: "#eef4f8",
            textShadow: "0 0 50px rgba(127,212,212,0.22)",
          }}
        >
          <CharReveal text="LanBeam" t={o} at={land + 0.45} stagger={0.045} />
        </div>
        <div
          style={{
            display: "inline-flex",
            alignItems: "center",
            marginTop: 14,
            fontFamily: MONO,
            fontSize: 22,
            fontWeight: 600,
            color: C.accentInk,
            background: C.accentSoft,
            border: "1px solid rgba(127,212,212,0.35)",
            borderRadius: 99,
            padding: "4px 14px",
            ...reveal(o, land + 0.95, Infinity, { dy: 10, blur: 6 }),
          }}
        >
          v0.2.0
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          top: 600,
          textAlign: "center",
          fontFamily: FONT,
          fontSize: 40,
          fontWeight: 500,
          letterSpacing: "0.12em",
          color: C.ink2,
          ...reveal(o, land + 1.1, Infinity, { dy: 16, blur: 10 }),
        }}
      >
        快速、私密、点对点的局域网文件传输
      </div>
      <div style={{ position: "absolute", left: 0, right: 0, top: 690, display: "flex", justifyContent: "center", gap: 18, fontFamily: FONT }}>
        {pills.map((p, i) => (
          <span
            key={p}
            style={{
              fontSize: 26,
              fontWeight: 500,
              letterSpacing: "0.1em",
              color: C.muted2,
              border: `1px solid ${C.border2}`,
              borderRadius: 99,
              padding: "8px 24px",
              ...reveal(o, land + 1.35 + i * 0.1, Infinity, { dy: 12, blur: 8 }),
            }}
          >
            {p}
          </span>
        ))}
      </div>
      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          top: 850,
          textAlign: "center",
          fontFamily: MONO,
          fontSize: 26,
          color: C.muted2,
          letterSpacing: "0.04em",
          ...reveal(o, land + 1.8, Infinity, { dy: 10, blur: 6 }),
        }}
      >
        github.com/Johnserf-Seed/lanbeam
      </div>
    </AbsoluteFill>
  );
}
