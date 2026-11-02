import { AbsoluteFill } from "remotion";
import { Cursor } from "../components/Cursor";
import { Caption, RollText, reveal } from "../components/Text";
import { E, lerp, prog, spr, SPRING } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";
import { RADAR_C, RADAR_Z } from "../lib/timeline";
import { ExtChip, Toast } from "../ui/primitives";
import { Radar, type RadarDevice } from "../ui/Radar";

// Device positions around the hub, measured off the real devices page.
export const DEVICES = [
  { id: "phone", name: "小敏的手机", ip: "192.168.1.41", x: 32, y: -167 },
  { id: "ipad", name: "iPad Air", ip: "192.168.1.57", x: 148, y: -19 },
  { id: "tp", name: "工位 · ThinkPad", ip: "192.168.1.36", x: 133, y: 100 },
  { id: "mini", name: "客厅 · Mac mini", ip: "192.168.1.23", x: -11, y: 139, mirror: true },
  { id: "nas", name: "NAS · Synology", ip: "192.168.1.9", x: -143, y: -32, mirror: true },
] as const;

const SWEEP_AT = 0.8; // local s
const DEG_PER_S = 120;
const angleOf = (x: number, y: number) => {
  const a = (Math.atan2(x, -y) * 180) / Math.PI;
  return a < 0 ? a + 360 : a;
};
export const foundAt = (x: number, y: number) => SWEEP_AT + angleOf(x, y) / DEG_PER_S;

const DROP = 5.05; // the file lands on the Mac mini

/** Radar state at local time u. */
export function radarState(u: number) {
  const hub = spr(u, 0.48, SPRING.pop);
  const rings = [0, 1, 2].map((i) => spr(u, 0.6 + i * 0.12, SPRING.soft)) as [number, number, number];
  const sweep = (u - SWEEP_AT) * DEG_PER_S;
  const devices: RadarDevice[] = DEVICES.map((d) => {
    const f = foundAt(d.x, d.y);
    const appear = spr(u, f, SPRING.pop);
    const isMini = d.id === "mini";
    const sending = isMini && u >= DROP + 0.2;
    const pulse = u >= f ? ((u - f) % 2.4) / 2.4 : -1;
    const target = isMini ? prog(u, DROP - 0.55, DROP - 0.3, E.outSoft) * (1 - prog(u, DROP + 0.05, DROP + 0.3)) : 0;
    return {
      ...d,
      appear,
      pulse: sending ? -1 : pulse,
      target,
      ring: sending ? 0 : undefined,
      note: sending
        ? { text: "发送中", color: C.accentInk, opacity: prog(u, DROP + 0.2, DROP + 0.5) }
        : { text: "刚刚发现", color: C.accentInk, opacity: prog(u, f + 0.15, f + 0.5) },
    };
  });
  const mini = DEVICES[3];
  const beamDraw = prog(u, DROP + 0.05, DROP + 0.6, E.out);
  return {
    hub,
    rings,
    sweep,
    sweepOpacity: prog(u, SWEEP_AT, SWEEP_AT + 0.6),
    hubPulse: u > 0.6 ? ((u - 0.6) % 3.4) / 3.4 : -1,
    selfNameOpacity: prog(u, 0.9, 1.4),
    devices,
    beam:
      u >= DROP
        ? { to: { x: mini.x, y: mini.y }, draw: beamDraw, dash: u * 2, label: "等待接收", labelOpacity: 0, opacity: 1 }
        : undefined,
  };
}

export function Discover({ t, exit }: { t: number; exit: number }) {
  const u = t;
  const st = radarState(u);

  // the drag: a file ghost riding the cursor onto the Mac mini
  const mini = DEVICES[3];
  const target = { x: RADAR_C.x + mini.x * RADAR_Z, y: RADAR_C.y + mini.y * RADAR_Z };
  const dragP = prog(u, 3.95, DROP - 0.1, E.inOut);
  const gx = lerp(250, target.x + 16, dragP);
  const gy = lerp(1010, target.y + 14, dragP) - Math.sin(dragP * Math.PI) * 90;
  const ghostIn = prog(u, 3.55, 3.95, E.outSoft);
  const ghostDrop = prog(u, DROP - 0.05, DROP + 0.25, E.in);

  // exit: push into the Mac mini while fading
  const zoom = 1 + 0.05 * prog(u, 0, 6, E.inOutSoft) + exit * 0.9;
  const ox = (RADAR_C.x - target.x) * exit * 0.9;
  const oy = (RADAR_C.y - target.y) * exit * 0.9;

  return (
    <AbsoluteFill>
      <div
        style={{
          position: "absolute",
          left: RADAR_C.x,
          top: RADAR_C.y,
          transform: `translate(${ox}px, ${oy}px) scale(${zoom})`,
          opacity: 1 - prog(exit, 0.2, 0.85),
          filter: exit > 0.05 ? `blur(${exit * 6}px)` : undefined,
        }}
      >
        <div style={{ zoom: RADAR_Z }}>
          <Radar
            rings={st.rings}
            sweep={st.sweep}
            sweepOpacity={st.sweepOpacity}
            hub={st.hub}
            hubPulse={st.hubPulse}
            selfName="书房 · MacBook Pro"
            selfNameOpacity={st.selfNameOpacity}
            devices={st.devices}
            beam={st.beam}
          />
        </div>
      </div>

      <div style={{ opacity: 1 - prog(exit, 0, 0.4) }}>
        <Caption
          t={u}
          at={1.0}
          out={5.6}
          index="01"
          label="自动发现"
          title="打开即见"
          sub={<>同一网络里的设备，<br />几秒内出现在雷达上</>}
          x={118}
          y={300}
          width={470}
        />
        <div
          style={{
            position: "absolute",
            left: 118,
            top: 640,
            fontFamily: MONO,
            fontSize: 21,
            color: C.muted2,
            display: "flex",
            alignItems: "center",
            gap: 12,
            ...reveal(u, 1.6, 5.6, { dy: 12, blur: 8 }),
          }}
        >
          <span style={{ width: 9, height: 9, borderRadius: "50%", background: C.accent, boxShadow: `0 0 10px ${C.glow}` }} />
          <span style={{ fontFamily: FONT }}>局域网 ·</span>
          <RollText
            t={u}
            height={30}
            style={{ color: C.accentInk, fontWeight: 600, minWidth: 16 }}
            dur={0.35}
            steps={[0, 1, 2, 3, 4, 5].map((n) => ({
              at: n === 0 ? 1.6 : foundAt(DEVICES[n - 1].x, DEVICES[n - 1].y) + 0.05,
              text: String(n),
            }))}
          />
          <span style={{ fontFamily: FONT }}>台在线</span>
        </div>
      </div>

      {/* file being dragged in */}
      {ghostIn > 0 && ghostDrop < 1 && (
        <div
          style={{
            position: "absolute",
            left: gx,
            top: gy,
            transform: `scale(${(0.9 + 0.1 * ghostIn) * (1 - 0.7 * ghostDrop)})`,
            transformOrigin: "0 0",
            opacity: ghostIn * (1 - ghostDrop) * 0.95,
            zIndex: 20,
          }}
        >
          <div
            style={{
              zoom: 1.6,
              display: "flex",
              alignItems: "center",
              gap: 9,
              padding: "7px 12px 7px 8px",
              borderRadius: 10,
              background: "rgba(23, 30, 40, 0.92)",
              border: `1px solid ${C.border2}`,
              boxShadow: "0 12px 30px rgba(0,0,0,0.5)",
              fontFamily: FONT,
            }}
          >
            <ExtChip ext="ZIP" size={26} fontSize={8} radius={7} />
            <div>
              <div style={{ fontSize: 12, fontWeight: 600, color: C.ink2, whiteSpace: "nowrap" }}>产品设计稿 v2.zip</div>
              <div style={{ fontSize: 10, color: C.muted }}>1.2 GB</div>
            </div>
          </div>
        </div>
      )}
      <Cursor
        t={u}
        show={[3.5, DROP + 0.7]}
        keys={[
          { t: 3.5, x: 236, y: 996 },
          { t: 3.95, x: 236, y: 996 },
          { t: DROP - 0.1, x: target.x + 2, y: target.y },
          { t: DROP + 0.7, x: target.x + 40, y: target.y + 40 },
        ]}
      />
      {/* the app's toast for a drop onto a device */}
      <div
        style={{
          position: "absolute",
          left: RADAR_C.x,
          top: 985,
          transform: "translateX(-50%)",
          ...reveal(u, DROP + 0.25, 6.0, { dy: 14, blur: 6, dur: 0.5 }),
          opacity: prog(u, DROP + 0.25, DROP + 0.6) * (1 - prog(exit, 0, 0.5)),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>开始发送 1 个文件 → 客厅 · Mac mini</Toast>
        </div>
      </div>
    </AbsoluteFill>
  );
}
