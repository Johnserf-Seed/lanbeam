import { AbsoluteFill } from "remotion";
import { Comet, trail } from "../components/Comet";
import { Cursor } from "../components/Cursor";
import { Fold } from "../components/Fold";
import { DeviceLabel } from "../components/Label";
import { Caption, reveal } from "../components/Text";
import { bez, E, prog, spr, SPRING } from "../lib/anim";
import { H, T, TEXT_C } from "../lib/timeline";
import { Toast } from "../ui/primitives";
import { InboxTextRow, QuickTextPanel } from "../ui/QuickText";

const Z = 1.75;
const PANEL_W = 440;
const PANEL_H = 326;
const TEXT = "https://figma.com/file/lb-review\n评审改到周四 10:30，帮我转给组里";
const RECV = { x: 1490, y: 650 };
const SEND_AT = 3.35;

export function QuickText({ t }: { t: number }) {
  const q = t;
  const open = H.sendText.t1 - T.text[0];
  const fold = H.textShare.fold - T.text[0];

  const typed = Math.floor(prog(q, 1.45, 2.85, (x) => x) * Array.from(TEXT).length);
  const text = Array.from(TEXT).slice(0, typed).join("");
  const typing = q > 1.4 && q < 2.95;
  const caret = typing || (q < SEND_AT && Math.floor(q * 2) % 2 === 0);
  const sendPress = q >= SEND_AT && q < SEND_AT + 0.22 ? Math.sin(((q - SEND_AT) / 0.22) * Math.PI) : 0;
  const lift = prog(q, SEND_AT + 0.05, SEND_AT + 0.35, E.in);

  const left = TEXT_C.x - (PANEL_W * Z) / 2;
  const top = TEXT_C.y - (PANEL_H * Z) / 2;

  // the text flies to the other screen
  const from = { x: left + 200 * Z, y: top + 120 * Z };
  const to = { x: RECV.x - 120, y: RECV.y + 6 };
  const flyAt = (s: number) => bez(from, { x: from.x + 260, y: from.y - 260 }, { x: to.x - 260, y: to.y - 220 }, to, prog(s, SEND_AT + 0.1, SEND_AT + 0.62, E.inOut));
  const flyOn = q > SEND_AT + 0.05 && q < SEND_AT + 0.85 ? 1 - prog(q, SEND_AT + 0.62, SEND_AT + 0.85) : 0;
  const arrive = SEND_AT + 0.6;
  const rIn = spr(q, arrive, SPRING.pop);
  const rOut = prog(q, fold - 0.1, fold + 0.25, E.in);

  return (
    <AbsoluteFill>
      <Caption
        t={q}
        at={open + 0.15}
        out={fold - 0.2}
        index="04"
        label="快传文本"
        title="直达对方剪贴板"
        sub="一段文字、一个链接，端到端加密，对方在线即达"
        x={left}
        y={92}
        width={1300}
      />
      <DeviceLabel style={{ left, top: top - 52, ...reveal(q, open + 0.35, fold - 0.1, { dy: 10, blur: 6 }) }}>
        本机 · 书房 · MacBook Pro
      </DeviceLabel>
      <Fold x={TEXT_C.x} y={TEXT_C.y} t={q} open={open} fold={fold} width={PANEL_W * Z}>
        <div style={{ zoom: Z }}>
          <QuickTextPanel
            text={text}
            caret={caret && q < SEND_AT}
            bytesLabel={typed > 0 ? "1 KB / 60 KB" : "0 KB / 60 KB"}
            clip={1}
            sendPress={sendPress}
            textLift={lift}
          />
        </div>
      </Fold>
      {flyOn > 0 && <Comet pts={trail(flyAt, q, 0.22, 26)} size={8} opacity={flyOn} />}

      {/* the other screen */}
      <DeviceLabel
        style={{
          left: RECV.x - (420 * 1.55) / 2,
          top: RECV.y - 130,
          ...reveal(q, arrive - 0.2, fold, { dy: 10, blur: 6 }),
        }}
      >
        客厅 · Mac mini
      </DeviceLabel>
      {rIn > 0.001 && (
        <div
          style={{
            position: "absolute",
            left: RECV.x,
            top: RECV.y,
            transform: `translate(-50%, -50%) scale(${0.85 + 0.15 * rIn})`,
            opacity: Math.min(1, rIn) * (1 - rOut),
            filter: rOut > 0 ? `blur(${rOut * 6}px)` : undefined,
          }}
        >
          <div style={{ zoom: 1.55 }}>
            <InboxTextRow badge={spr(q, arrive + 0.35, SPRING.pop)} />
          </div>
        </div>
      )}
      <div
        style={{
          position: "absolute",
          left: RECV.x,
          top: RECV.y + 120,
          transform: "translateX(-50%)",
          ...reveal(q, arrive + 0.15, fold - 0.15, { dy: 12, blur: 6, dur: 0.45 }),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>收到来自 书房 · MacBook Pro 的文本</Toast>
        </div>
      </div>
      <Cursor
        t={q}
        show={[2.35, SEND_AT + 0.75]}
        keys={[
          { t: 2.35, x: 1180, y: 990 },
          { t: SEND_AT, x: left + 385 * Z, y: top + 257 * Z, click: true },
          { t: SEND_AT + 0.75, x: left + 420 * Z, y: top + 290 * Z },
        ]}
      />
    </AbsoluteFill>
  );
}
