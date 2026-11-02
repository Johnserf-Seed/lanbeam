import { AbsoluteFill } from "remotion";
import { Comet, trail } from "../components/Comet";
import { Cursor } from "../components/Cursor";
import { Fold } from "../components/Fold";
import { DeviceLabel } from "../components/Label";
import { Caption, RollDigits, reveal } from "../components/Text";
import { bez, E, lerp, prog, spr, SPRING } from "../lib/anim";
import { C } from "../lib/theme";
import { H, SEND_C, T } from "../lib/timeline";
import { Toast } from "../ui/primitives";
import { IncomingCard } from "../ui/Incoming";
import { TransferCard } from "../ui/TransferCard";

const Z = 1.55; // the transfer card's zoom
const CARD_W = 600;
const P1 = { x: 575, y: 620 }; // while the codes are compared
const RZ = 1.7; // the incoming card's zoom
const R = { x: 1455, y: 610 };

const ACCEPT = 3.6;
const START = 4.15;
const PAUSE = 6.45;
const RESUME = 7.6;
const DONE = 9.2;

function pctAt(v: number) {
  if (v < START) return 0;
  if (v < PAUSE) return 46 * prog(v, START, PAUSE, E.inOutSoft);
  if (v < RESUME) return 46;
  return 46 + 54 * prog(v, RESUME, DONE, E.inOutSoft);
}

function speedAt(v: number) {
  if (v < START || (v >= PAUSE && v < RESUME) || v >= DONE) return 0;
  const base = v < PAUSE ? 48.6 : 55.2;
  const ramp = v < PAUSE ? prog(v, START, START + 0.4) : prog(v, RESUME, RESUME + 0.3);
  return (base + 3.2 * Math.sin(v * 7.1) + 2.1 * Math.sin(v * 13.3 + 1)) * ramp;
}

function etaAt(v: number) {
  const s = speedAt(v);
  if (s < 1) return "";
  const sec = Math.ceil((1229 * (1 - pctAt(v) / 100)) / s);
  return `剩余 ${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}`;
}

export function Send({ t }: { t: number }) {
  const v = t;
  const fold = H.sendText.fold - T.send[0];

  // where the card is: beside the receiver, then centre stage
  const move = prog(v, 4.55, 5.45, E.inOut);
  const cx = lerp(P1.x, SEND_C.x, move);
  const cy = lerp(P1.y, SEND_C.y, move);
  const scale = lerp(1, 1.3, move);

  const pct = pctAt(v);
  const speed = speedAt(v);
  const hist: number[] = [];
  if (v > START) for (let k = 39; k >= 0; k--) {
    const s = v - k * 0.1;
    if (s >= START) hist.push(speedAt(s));
  }
  const paused = v >= PAUSE && v < RESUME;
  const pausePress = Math.max(
    v >= PAUSE && v < PAUSE + 0.22 ? Math.sin(((v - PAUSE) / 0.22) * Math.PI) : 0,
    v >= RESUME && v < RESUME + 0.22 ? Math.sin(((v - RESUME) / 0.22) * Math.PI) : 0,
  );

  // the codes: same digits, rolling in on both screens
  const sasAt = 1.25;
  const sas = (lh: number) => <RollDigits value="483 · 921" t={v} at={sasAt} lineHeight={lh} dur={0.95} />;
  const glow = prog(v, 2.45, 2.75, E.outSoft) * (1 - prog(v, 3.9, 4.3));
  const awaiting = v < START - 0.1;
  const status = awaiting ? (
    <span>
      已在对方屏幕上 · 校验码{" "}
      <span
        style={{
          display: "inline-flex",
          verticalAlign: "bottom",
          color: C.accentInk,
          fontWeight: 600,
          borderRadius: 4,
          padding: "0 3px",
          margin: "0 -3px",
          background: `rgba(127,212,212,${0.14 * glow})`,
          boxShadow: glow ? `0 0 0 1px rgba(127,212,212,${0.7 * glow}), 0 0 16px rgba(127,212,212,${0.4 * glow})` : undefined,
        }}
      >
        {sas(15.75)}
      </span>
    </span>
  ) : paused ? (
    "已暂停"
  ) : v >= DONE ? (
    <span style={{ color: C.success }}>已完成</span>
  ) : (
    "传输中"
  );

  // receiver card: in like the app's lbCard, out like lbCardOut
  const rIn = prog(v, 0.75, 1.1, E.outSoft);
  const rOut = prog(v, ACCEPT + 0.25, ACCEPT + 0.55, E.in);
  const acceptPress = v >= ACCEPT && v < ACCEPT + 0.22 ? Math.sin(((v - ACCEPT) / 0.22) * Math.PI) : 0;

  // the light that checks one code against the other
  const from = { x: 652, y: 690 };
  const to = { x: 1455, y: 630 };
  const c1 = { x: 820, y: 930 };
  const c2 = { x: 1330, y: 900 };
  const runAt = (s: number) => bez(from, c1, c2, to, prog(s, 2.5, 3.05, E.inOut));
  const runOn = v >= 2.45 && v < 3.3 ? 1 - prog(v, 3.05, 3.3) : 0;
  const match = spr(v, 3.02, SPRING.pop) * (1 - prog(v, 3.85, 4.15));

  const doneGlow = prog(v, DONE, DONE + 0.3) * (1 - prog(v, DONE + 0.5, DONE + 1.0));

  return (
    <AbsoluteFill>
      <Caption
        t={v}
        at={0.55}
        out={3.95}
        index="02"
        label="端到端加密"
        title="核对校验码，一致再接收"
        sub="Noise_XX 双向认证 · 前向保密 · 每台设备独立会话"
        x={960}
        y={96}
        width={1400}
        align="center"
      />
      <Caption
        t={v}
        at={4.95}
        out={fold - 0.2}
        index="03"
        label="文件与文件夹"
        title="断点续传"
        sub="暂停、继续，中断后从断点接着传 · SHA-256 完整性校验"
        x={960}
        y={120}
        width={1400}
        align="center"
      />

      {/* sender */}
      <DeviceLabel
        style={{
          left: P1.x - (CARD_W * Z) / 2,
          top: P1.y - 70 * Z - 52,
          ...reveal(v, 0.35, 4.2, { dy: 10, blur: 6 }),
        }}
      >
        本机 · 书房 · MacBook Pro
      </DeviceLabel>
      <Fold x={cx} y={cy} t={v} open={0.15} fold={fold} width={CARD_W * Z * scale}>
        <div style={{ transform: `scale(${scale})`, filter: doneGlow ? `drop-shadow(0 0 ${24 * doneGlow}px rgba(127,191,147,${0.5 * doneGlow}))` : undefined }}>
          <div style={{ zoom: Z }}>
            <TransferCard
              width={CARD_W}
              pct={pct}
              speed={speed}
              eta={etaAt(v)}
              status={status}
              paused={paused}
              hist={hist}
              pausePress={pausePress}
              done={prog(v, DONE, DONE + 0.3)}
            />
          </div>
        </div>
      </Fold>

      {/* receiver */}
      <DeviceLabel
        style={{
          left: R.x - (302 * RZ) / 2,
          top: R.y - 115 * RZ - 60,
          ...reveal(v, 0.6, ACCEPT + 0.9, { dy: 10, blur: 6 }),
        }}
      >
        客厅 · Mac mini
      </DeviceLabel>
      {rIn > 0 && rOut < 1 && (
        <div
          style={{
            position: "absolute",
            left: R.x,
            top: R.y,
            transform: `translate(-50%, -50%) translateY(${(1 - rIn) * 14 * RZ - rOut * 14 * RZ}px)`,
            opacity: rIn * (1 - rOut),
          }}
        >
          <div style={{ zoom: RZ }}>
            <IncomingCard
              peer="书房 · MacBook Pro"
              sub="请求发送 1 个文件 · 1.2 GB"
              file="产品设计稿 v2.zip"
              sas={sas(21)}
              sasGlow={glow}
              acceptPress={acceptPress}
            />
          </div>
        </div>
      )}
      <div
        style={{
          position: "absolute",
          left: R.x,
          top: R.y,
          transform: "translate(-50%, -50%)",
          ...reveal(v, ACCEPT + 0.4, 4.45, { dy: 12, blur: 6, dur: 0.45 }),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>开始接收来自 书房 · MacBook Pro 的文件</Toast>
        </div>
      </div>

      {/* code check */}
      {runOn > 0 && <Comet pts={trail(runAt, v, 0.2, 24)} size={7} opacity={runOn} />}
      {match > 0.01 && (
        <div
          style={{
            position: "absolute",
            left: 1080,
            top: 878,
            transform: `translate(-50%, -50%) scale(${0.6 + 0.4 * match})`,
            opacity: Math.min(1, match),
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "10px 22px",
            borderRadius: 99,
            background: "rgba(127,212,212,0.14)",
            border: "1px solid rgba(127,212,212,0.55)",
            boxShadow: "0 0 30px rgba(127,212,212,0.25)",
            color: C.accentInk,
            fontSize: 24,
            fontWeight: 600,
            letterSpacing: "0.08em",
            whiteSpace: "nowrap",
          }}
        >
          ✓ 校验码一致
        </div>
      )}

      <Cursor
        t={v}
        show={[3.0, 8.3]}
        keys={[
          { t: 3.0, x: 1760, y: 1010 },
          { t: ACCEPT, x: 1573, y: 757, click: true },
          { t: PAUSE, x: 1391, y: 593, click: true },
          { t: RESUME, x: 1391, y: 593, click: true },
          { t: 8.3, x: 1500, y: 760 },
        ]}
      />
      <div
        style={{
          position: "absolute",
          left: SEND_C.x,
          top: 830,
          transform: "translateX(-50%)",
          ...reveal(v, PAUSE + 0.12, RESUME + 0.05, { dy: 12, blur: 6, dur: 0.45 }),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>已暂停 · 若长时间无人操作会自动继续</Toast>
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: SEND_C.x,
          top: 830,
          transform: "translateX(-50%)",
          ...reveal(v, DONE + 0.1, fold - 0.15, { dy: 12, blur: 6, dur: 0.45 }),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>已发送到 客厅 · Mac mini · 产品设计稿 v2.zip</Toast>
        </div>
      </div>
    </AbsoluteFill>
  );
}
