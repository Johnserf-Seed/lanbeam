import { AbsoluteFill } from "remotion";
import { Cursor } from "../components/Cursor";
import { Caption, reveal } from "../components/Text";
import { bez, E, prog, spr, SPRING } from "../lib/anim";
import { H, T, TRUST_C } from "../lib/timeline";
import { Toast } from "../ui/primitives";
import { TRUST_R, TrustCircle, type TrustNode } from "../ui/TrustCircle";

const Z = 2.0;
const GRAB = 1.78;
const DROP = 2.78;
const PHONE_FROM = { x: -262, y: 104 };
const PHONE_TO = { x: -58, y: 72 };

export function Trust({ t }: { t: number }) {
  const r = t;
  const open = H.pairTrust.t1 - T.trust[0];
  const fold = H.trustLook.fold - T.trust[0];

  const ring = spr(r, open, SPRING.soft);
  const out = prog(r, fold, fold + 0.45, E.inOut);

  const dragU = prog(r, GRAB + 0.08, DROP, E.inOut);
  const phonePos = bez(
    PHONE_FROM,
    { x: PHONE_FROM.x + 40, y: PHONE_FROM.y - 90 },
    { x: PHONE_TO.x - 60, y: PHONE_TO.y - 80 },
    PHONE_TO,
    dragU,
  );
  const lift = prog(r, GRAB, GRAB + 0.15) * (1 - prog(r, DROP, DROP + 0.2));
  const inside = Math.hypot(phonePos.x, phonePos.y) < TRUST_R;
  const hot = (inside ? prog(r, GRAB + 0.5, GRAB + 0.7) : 0) * (1 - prog(r, DROP + 0.1, DROP + 0.5));
  const trusted = prog(r, DROP + 0.05, DROP + 0.4, E.outSoft);

  const nodeIn = (i: number) => spr(r, open + 0.25 + i * 0.08, SPRING.pop);
  const nodes: TrustNode[] = [
    { id: "mini", name: "客厅 · Mac mini", x: -68, y: -58, trusted: 1, auto: true, mirror: true },
    { id: "nas", name: "NAS · Synology", x: 66, y: 58, trusted: 1, auto: false },
    { id: "ipad", name: "iPad Air", x: 44, y: -212, trusted: 0, auto: false },
    { id: "tp", name: "工位 · ThinkPad", x: 214, y: -60, trusted: 0, auto: false },
    { id: "phone", name: "小敏的手机", x: phonePos.x, y: phonePos.y, trusted, auto: true, mirror: true, lift },
  ].map((n, i) => ({ ...n, appear: nodeIn(i) }));

  const sc = (1 - out) * (0.92 + 0.08 * Math.min(1, ring));
  const screen = (x: number, y: number) => ({ x: TRUST_C.x + x * Z * sc, y: TRUST_C.y + y * Z * sc });
  const grab = screen(PHONE_FROM.x, PHONE_FROM.y);
  const drop = screen(PHONE_TO.x, PHONE_TO.y);

  return (
    <AbsoluteFill>
      <Caption
        t={r}
        at={open + 0.15}
        out={fold - 0.2}
        index="07"
        label="信任圈"
        title="拖进圈内，自动接收"
        sub={<>自己的设备，<br />以后不必每次确认</>}
        x={118}
        y={360}
        width={560}
      />
      <div
        style={{
          position: "absolute",
          left: TRUST_C.x,
          top: TRUST_C.y,
          transform: `scale(${sc})`,
          opacity: Math.min(1, ring * 1.5) * (1 - prog(out, 0.6, 1)),
          filter: out > 0 ? `brightness(${1 + out * 1.5})` : undefined,
        }}
      >
        <div style={{ zoom: Z }}>
          <TrustCircle nodes={nodes} hot={hot} ring={Math.min(1, ring)} />
        </div>
      </div>
      <Cursor
        t={r}
        show={[1.2, DROP + 0.6]}
        keys={[
          { t: 1.2, x: grab.x - 160, y: grab.y + 200 },
          { t: GRAB, x: grab.x + 2, y: grab.y + 2 },
          { t: DROP, x: drop.x + 2, y: drop.y + 2 },
          { t: DROP + 0.6, x: drop.x + 60, y: drop.y + 90 },
        ]}
      />
      <div
        style={{
          position: "absolute",
          left: TRUST_C.x,
          top: TRUST_C.y + TRUST_R * Z + 70,
          transform: "translateX(-50%)",
          ...reveal(r, DROP + 0.2, fold - 0.1, { dy: 12, blur: 6, dur: 0.45 }),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>已将 小敏的手机 加入信任圈 · 以后自动接收，不再询问</Toast>
        </div>
      </div>
    </AbsoluteFill>
  );
}
