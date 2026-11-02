import { AbsoluteFill } from "remotion";
import { BrandMark, MARK_DOT } from "../components/BrandMark";
import { Comet, trail } from "../components/Comet";
import { CharReveal, reveal } from "../components/Text";
import { E, type Flight, flightAt, lerp, prog, spr, SPRING } from "../lib/anim";
import { C, FONT } from "../lib/theme";
import { RADAR_C } from "../lib/timeline";

const S = 230; // mark size
const Y = 500; // mark centre
const X0 = 960;
const X1 = 676; // after making room for the wordmark

const markX = (t: number) => lerp(X0, X1, prog(t, 2.05, 2.8, E.inOut));
const dotAt = (t: number) => ({
  x: markX(t) - S / 2 + MARK_DOT.x * S,
  y: Y - S / 2 + MARK_DOT.y * S,
});

const LAND = 1.15;
const LEAVE = 4.12;
const ARRIVE = 4.78;

// In from the top right, settle on the mark's dot; at the end, the dot leaves
// for the radar hub.
const flights = (): Flight[] => {
  const d0 = dotAt(0);
  const d1 = dotAt(LEAVE);
  return [
    {
      t0: 0.05,
      t1: LAND,
      p: [{ x: 2080, y: -170 }, { x: 1560, y: 30 }, { x: 1080, y: 250 }, d0],
      ease: E.outSoft,
    },
    { t0: LAND, t1: LAND + 0.01, p: [d0, d0, d0, d0] },
    {
      t0: LEAVE,
      t1: ARRIVE,
      p: [d1, { x: d1.x + 120, y: d1.y + 170 }, { x: RADAR_C.x - 200, y: RADAR_C.y + 60 }, RADAR_C],
      ease: E.inOut,
    },
  ];
};

export function Intro({ t }: { t: number }) {
  const fl = flights();
  // While parked on the mark, the light rides along with it.
  const at = (x: number) => (x >= LAND + 0.01 && x < LEAVE ? dotAt(x) : flightAt(fl, x));
  const cometOn = t < LAND ? 1 : t < LEAVE ? 0 : 1 - prog(t, ARRIVE - 0.02, ARRIVE + 0.25);

  const flash = prog(t, LAND - 0.02, LAND + 0.7, E.out);
  const square = spr(t, LAND + 0.18, SPRING.soft) * (1 - prog(t, 3.95, 4.35, E.in));
  const dot = t < LAND ? 0 : t < LEAVE ? spr(t, LAND - 0.04, SPRING.pop) : 0;
  const arcs: [number, number, number] = [0, 1, 2].map(
    (i) => prog(t, LAND + 0.1 + i * 0.13, LAND + 0.55 + i * 0.13, E.out) * (1 - prog(t, 3.9 + i * 0.05, 4.2 + i * 0.05, E.in)),
  ) as [number, number, number];

  const mx = markX(t);
  const d = dotAt(t);
  const textOut = 3.85;

  return (
    <AbsoluteFill>
      {/* landing flash */}
      {flash > 0 && flash < 1 && (
        <div
          style={{
            position: "absolute",
            left: d.x,
            top: d.y,
            width: lerp(20, 760, flash),
            height: lerp(20, 760, flash),
            transform: "translate(-50%, -50%)",
            borderRadius: "50%",
            background: "radial-gradient(circle, rgba(196,250,250,0.55) 0%, rgba(127,212,212,0.18) 35%, rgba(127,212,212,0) 70%)",
            opacity: 1 - flash,
          }}
        />
      )}
      {flash > 0 && flash < 1 && (
        <div
          style={{
            position: "absolute",
            left: d.x,
            top: d.y,
            width: lerp(10, 520, flash),
            height: lerp(10, 520, flash),
            transform: "translate(-50%, -50%)",
            borderRadius: "50%",
            border: `1.5px solid rgba(147,222,222,${0.6 * (1 - flash)})`,
          }}
        />
      )}
      <div style={{ position: "absolute", left: mx - S / 2, top: Y - S / 2 }}>
        <BrandMark size={S} square={square} dot={dot} arcs={arcs} glow={1} />
      </div>
      {/* wordmark + tagline */}
      <div
        style={{
          position: "absolute",
          left: X1 + S / 2 + 48,
          top: Y - 112,
          fontFamily: FONT,
        }}
      >
        <div
          style={{
            fontSize: 138,
            fontWeight: 600,
            letterSpacing: "-0.02em",
            lineHeight: 1,
            color: "#eef4f8",
            textShadow: "0 0 50px rgba(127,212,212,0.22)",
          }}
        >
          <CharReveal text="LanBeam" t={t} at={2.32} out={textOut} stagger={0.045} />
        </div>
        <div
          style={{
            marginTop: 26,
            fontSize: 38,
            fontWeight: 500,
            letterSpacing: "0.18em",
            color: C.muted2,
            ...reveal(t, 2.9, textOut + 0.1, { dy: 16, blur: 10 }),
          }}
        >
          一束即达 · 无需云端
        </div>
      </div>
      <Comet pts={trail(at, t, 0.26, 30)} size={9} opacity={cometOn} />
    </AbsoluteFill>
  );
}
