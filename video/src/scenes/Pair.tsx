import { AbsoluteFill } from "remotion";
import { Cursor } from "../components/Cursor";
import { Fold } from "../components/Fold";
import { DeviceLabel } from "../components/Label";
import { Caption, RollDigits, RollText, reveal } from "../components/Text";
import { E, lerp, prog, spr, SPRING } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";
import { H, PAIR_C, T } from "../lib/timeline";
import { PairPanel } from "../ui/Pair";

const Z = 1.5;
const W = 500;
// panel heights (CSS px) per view, so the light line and labels line up
const HOST_H = 413;
const JOIN_H = 326;
const FLIP = 2.55;
const PASTE = 3.55;
const JOIN = 4.25;
const COMPARE = 4.5;

export function Pair({ t }: { t: number }) {
  const p = t;
  const open = H.sharePair.t1 - T.pair[0];
  const fold = H.pairTrust.fold - T.pair[0];
  const left = PAIR_C.x - (W * Z) / 2;

  // a card flip: the MacBook's screen turns into the Mac mini's
  const f = prog(p, FLIP, FLIP + 0.6, E.inOut);
  const angle = f * 180;
  const back = angle >= 90;
  const view: "host" | "join" | "compare" = !back ? "host" : p < COMPARE ? "join" : "compare";
  const swap = prog(p, COMPARE - 0.05, COMPARE + 0.3, E.outSoft);
  const panelH = back ? JOIN_H : HOST_H;

  const remaining = `剩余 ${p < 2.0 ? "10:00" : `9:${String(60 - Math.floor(p - 1)).padStart(2, "0")}`}`;
  const pasted = p >= PASTE;
  const joinPress = p >= JOIN && p < JOIN + 0.22 ? Math.sin(((p - JOIN) / 0.22) * Math.PI) : 0;

  const topOf = (h: number) => PAIR_C.y - (h * Z) / 2;

  return (
    <AbsoluteFill>
      <Caption
        t={p}
        at={open + 0.15}
        out={fold - 0.2}
        index="06"
        label="跨网段配对"
        title="地址 + 6 位配对码"
        sub={<>自动发现看不到对方时也能连上，<br />也可以直接粘贴配对链接</>}
        x={1196}
        y={360}
        width={640}
      />
      <DeviceLabel
        style={{
          left,
          top: topOf(panelH) - 54,
          ...reveal(p, open + 0.35, fold - 0.1, { dy: 10, blur: 6 }),
        }}
      >
        <RollText
          t={p}
          height={30}
          steps={[
            { at: 0, text: "书房 · MacBook Pro · 让对方加入我" },
            { at: FLIP + 0.25, text: "客厅 · Mac mini · 加入对方" },
          ]}
        />
      </DeviceLabel>
      <Fold x={PAIR_C.x} y={PAIR_C.y} t={p} open={open} fold={fold} width={W * Z}>
        <div style={{ perspective: 1800 }}>
          <div
            style={{
              transform: `rotateY(${back ? angle - 180 : angle}deg)`,
              transformOrigin: "50% 50%",
              filter: f > 0 && f < 1 ? `brightness(${1 - 0.35 * Math.sin(f * Math.PI)})` : undefined,
            }}
          >
            <div style={{ zoom: Z, position: "relative" }}>
              <div style={{ opacity: view === "compare" ? 0 : 1 }}>
                {view !== "compare" && (
                  <PairPanel
                    role={back ? 1 : 0}
                    view={back ? "join" : "host"}
                    addr={
                      <span style={reveal(p, open + 0.4, Infinity, { dy: 6, blur: 6 })}>
                        192.168.1.20<span style={{ color: C.muted }}>:51704</span>
                      </span>
                    }
                    code={<RollDigits value="482 913" t={p} at={open + 0.55} lineHeight={27} dur={1.0} />}
                    remaining={remaining}
                    joinAddr={pasted ? "192.168.1.20:51704" : ""}
                    joinCode={pasted ? "482 913" : ""}
                    linkChip={spr(p, PASTE + 0.05, SPRING.pop)}
                    joinPress={joinPress}
                  />
                )}
              </div>
              {view === "compare" && (
                <div style={{ opacity: swap, transform: `translateY(${(1 - swap) * 8}px)` }}>
                  <PairPanel
                    role={1}
                    view="compare"
                    addr=""
                    code=""
                    remaining=""
                    joinAddr=""
                    joinCode=""
                    linkChip={0}
                    compareSas={<RollDigits value="736 · 205" t={p} at={COMPARE + 0.1} lineHeight={34} dur={0.9} />}
                  />
                </div>
              )}
            </div>
          </div>
        </div>
      </Fold>
      {/* ⌘V: the pair link is pasted into the address field */}
      <div
        style={{
          position: "absolute",
          left: left - 150,
          top: topOf(JOIN_H) + 156 * Z,
          display: "flex",
          gap: 8,
          fontFamily: FONT,
          ...reveal(p, PASTE - 0.3, PASTE + 0.45, { dy: 14, blur: 6, dur: 0.35 }),
          transform: `scale(${p >= PASTE && p < PASTE + 0.18 ? 0.92 : 1})`,
        }}
      >
        {["⌘", "V"].map((k) => (
          <span
            key={k}
            style={{
              minWidth: 52,
              height: 52,
              padding: "0 12px",
              borderRadius: 12,
              background: "linear-gradient(180deg, #2a3340, #1a212b)",
              border: `1px solid ${C.border2}`,
              boxShadow: "0 6px 0 #0c1117, 0 10px 24px rgba(0,0,0,0.5)",
              color: C.ink2,
              fontSize: 26,
              fontWeight: 600,
              fontFamily: k === "V" ? MONO : FONT,
              display: "grid",
              placeItems: "center",
            }}
          >
            {k}
          </span>
        ))}
      </div>
      <Cursor
        t={p}
        show={[3.75, COMPARE + 0.35]}
        keys={[
          { t: 3.75, x: lerp(left + 300 * Z, 1180, 0.3), y: 980 },
          { t: JOIN, x: left + 454 * Z, y: topOf(JOIN_H) + 180 * Z, click: true },
          { t: COMPARE + 0.35, x: left + 470 * Z, y: topOf(JOIN_H) + 230 * Z },
        ]}
      />
    </AbsoluteFill>
  );
}
