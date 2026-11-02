import { AbsoluteFill, Img, staticFile } from "remotion";
import { RollText, reveal } from "../components/Text";
import { E, lerp, prog } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";
import { H, LOOK_C, T } from "../lib/timeline";

// The real app (captured from the ?demo=1 build by scripts/capture-ui.mjs).
const IMG_W = 1180;
const IMG_H = (IMG_W * 1600) / 2560;
const DARK = 1.9;
const EN = 3.65;
const OS = 5.0;

/** One shot wiping over the previous one, with the beam riding the edge. */
function Wipe({ p, src }: { p: number; src: string }) {
  if (p <= 0) return null;
  const slant = 0.22 * IMG_H;
  const x = lerp(-slant, IMG_W + slant, p);
  const poly = `polygon(0 0, ${x + slant}px 0, ${x - slant}px ${IMG_H}px, 0 ${IMG_H}px)`;
  return (
    <>
      <Img
        src={staticFile(src)}
        style={{ position: "absolute", inset: 0, width: IMG_W, height: IMG_H, clipPath: p < 1 ? poly : undefined }}
      />
      {p < 1 && (
        <svg width={IMG_W} height={IMG_H} style={{ position: "absolute", inset: 0, overflow: "visible" }}>
          <defs>
            <filter id="wipeglow" x="-50%" y="-50%" width="200%" height="200%">
              <feGaussianBlur stdDeviation="7" />
            </filter>
          </defs>
          <line x1={x + slant} y1={0} x2={x - slant} y2={IMG_H} stroke={C.accent} strokeWidth={10} filter="url(#wipeglow)" opacity={0.9} />
          <line x1={x + slant} y1={0} x2={x - slant} y2={IMG_H} stroke="#ffffff" strokeWidth={2.5} />
        </svg>
      )}
    </>
  );
}

export function Look({ t }: { t: number }) {
  const k = t;
  const open = H.trustLook.t1 - T.look[0];
  const fold = H.lookOutro.fold - T.look[0];

  const inP = prog(k, open, open + 0.75, E.out);
  const outP = prog(k, fold, fold + 0.45, E.in);
  const scale = lerp(0.18, 1, inP) * lerp(1, 0.02, outP);
  const ry = lerp(-24, -13, inP) + lerp(0, 5, prog(k, open, fold, E.inOutSoft));
  const rx = lerp(10, 5, inP);
  const glow = (1 - inP) + outP;

  const title = [
    { at: open + 0.25, text: "深色模式" },
    { at: EN + 0.1, text: "中文 / English" },
    { at: OS, text: "三平台通用" },
  ];
  const sub = [
    { at: open + 0.45, text: "默认跟随系统，也能手动切换" },
    { at: EN + 0.25, text: "界面语言随时切换" },
    { at: OS + 0.15, text: "Windows · macOS · Linux" },
  ];
  const capOut = fold - 0.2;

  return (
    <AbsoluteFill>
      <div style={{ position: "absolute", left: 118, top: 372, fontFamily: FONT, width: 560 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 14, ...reveal(k, open + 0.1, capOut, { dy: 14, blur: 8 }) }}>
          <span style={{ fontFamily: MONO, fontSize: 21, fontWeight: 600, color: C.accent, letterSpacing: "0.08em" }}>08</span>
          <span style={{ width: 34, height: 1.5, background: C.accent, opacity: 0.55 }} />
          <span style={{ fontSize: 23, fontWeight: 500, color: C.muted2, letterSpacing: "0.16em" }}>外观与语言</span>
        </div>
        <div
          style={{
            marginTop: 18,
            fontSize: 68,
            lineHeight: 1.15,
            fontWeight: 600,
            letterSpacing: "0.02em",
            color: "#eef4f8",
            textShadow: "0 0 40px rgba(127, 212, 212, 0.18)",
            ...reveal(k, open + 0.2, capOut, { dy: 0, blur: 0, dur: 0.01 }),
          }}
        >
          <RollText t={k} height={80} steps={title} dur={0.7} />
        </div>
        <div style={{ marginTop: 20, fontSize: 26, lineHeight: 1.6, color: C.muted2, ...reveal(k, open + 0.4, capOut, { dy: 0, blur: 0, dur: 0.01 }) }}>
          <RollText t={k} height={42} steps={sub} dur={0.6} />
        </div>
      </div>

      <div
        style={{
          position: "absolute",
          left: LOOK_C.x,
          top: LOOK_C.y,
          width: 0,
          height: 0,
          perspective: 2600,
        }}
      >
        <div
          style={{
            position: "absolute",
            left: -IMG_W / 2,
            top: -IMG_H / 2,
            width: IMG_W,
            height: IMG_H,
            transform: `scale(${scale}) rotateY(${ry}deg) rotateX(${rx}deg)`,
            opacity: Math.min(1, inP * 2) * (1 - prog(outP, 0.7, 1)),
            borderRadius: 12,
            overflow: "hidden",
            boxShadow: "0 0 0 1px rgba(255,255,255,0.10), 0 50px 120px rgba(0,0,0,0.7), 0 0 80px rgba(127,212,212,0.08)",
            filter: glow > 0.02 ? `brightness(${1 + glow * 1.6}) blur(${glow * 4}px)` : undefined,
          }}
        >
          <Img src={staticFile("ui/devices-zh-light.webp")} style={{ position: "absolute", inset: 0, width: IMG_W, height: IMG_H }} />
          <Wipe p={prog(k, DARK, DARK + 0.85, E.inOut)} src="ui/devices-zh-dark.webp" />
          <Wipe p={prog(k, EN, EN + 0.85, E.inOut)} src="ui/devices-en-dark.webp" />
        </div>
      </div>
    </AbsoluteFill>
  );
}
