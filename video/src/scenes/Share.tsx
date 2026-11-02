import { AbsoluteFill } from "remotion";
import { Comet, trail } from "../components/Comet";
import { Fold } from "../components/Fold";
import { DeviceLabel } from "../components/Label";
import { Caption, RollText, reveal } from "../components/Text";
import { bez, E, prog, spr, SPRING } from "../lib/anim";
import { H, SHARE_C, T } from "../lib/timeline";
import { Toast } from "../ui/primitives";
import { Phone, SHARE_URL, SharePanel } from "../ui/Share";

const Z = 1.75;
const PANEL_W = 440;
const PANEL_H = 323;
const PHONE = { x: 470, y: 585 };
const PZ = 1.2;
const TAP = 3.95;
const DL = 4.45;

export function Share({ t }: { t: number }) {
  const s = t;
  const open = H.textShare.t1 - T.share[0];
  const fold = H.sharePair.fold - T.share[0];
  const left = SHARE_C.x - (PANEL_W * Z) / 2;
  const top = SHARE_C.y - (PANEL_H * Z) / 2;

  const linkChars = Math.floor(prog(s, open + 0.25, open + 1.05, (x) => x) * SHARE_URL.length);
  const qrAt = open + 0.45;

  const phoneIn = spr(s, 2.0, SPRING.soft);
  const phoneOut = prog(s, fold - 0.45, fold + 0.05, E.in);

  // the scan: from the code on screen into the phone
  const qr = { x: left + 378 * Z, y: top + 113 * Z };
  const scr = { x: PHONE.x, y: PHONE.y - 40 };
  const scanAt = (x: number) => bez(qr, { x: qr.x - 200, y: qr.y - 260 }, { x: scr.x + 260, y: scr.y - 300 }, scr, prog(x, 2.6, 3.1, E.inOut));
  const scanOn = s > 2.55 && s < 3.35 ? 1 - prog(s, 3.1, 3.35) : 0;

  const tap = s >= TAP && s < TAP + 0.6 ? (s - TAP) / 0.6 : -1;
  const finger = prog(s, TAP - 0.3, TAP - 0.05, E.outSoft) * (1 - prog(s, TAP + 0.2, TAP + 0.5));

  return (
    <AbsoluteFill>
      <Caption
        t={s}
        at={open + 0.15}
        out={fold - 0.2}
        index="05"
        label="浏览器接收"
        title="没装 App，扫码就能取"
        sub="一次性链接 + 二维码 · 限时限次 · 下载支持断点续传"
        x={left}
        y={92}
        width={1100}
      />
      <Fold x={SHARE_C.x} y={SHARE_C.y} t={s} open={open} fold={fold} width={PANEL_W * Z}>
        <div style={{ zoom: Z }}>
          <SharePanel
            t={s}
            linkChars={linkChars}
            qrAt={qrAt}
            downloads={
              <span>
                {"已下载 "}
                <RollText t={s} height={17} dur={0.4} steps={[{ at: 0, text: "0" }, { at: DL, text: "1" }]} />
                {" 次"}
              </span>
            }
          />
        </div>
      </Fold>
      {scanOn > 0 && <Comet pts={trail(scanAt, s, 0.22, 26)} size={8} opacity={scanOn} />}

      {/* the phone, without LanBeam */}
      <DeviceLabel
        style={{
          left: PHONE.x - 150 * PZ,
          top: PHONE.y - 310 * PZ - 54,
          ...reveal(s, 2.2, fold - 0.2, { dy: 10, blur: 6 }),
        }}
      >
        小敏的手机 · 没装 LanBeam
      </DeviceLabel>
      {phoneIn > 0.001 && (
        <div
          style={{
            position: "absolute",
            left: PHONE.x,
            top: PHONE.y,
            transform: `translate(-50%, -50%) translateY(${(1 - phoneIn) * 420 + phoneOut * 160}px) perspective(1800px) rotateY(${(1 - phoneIn) * 22 + 6}deg) rotateX(${(1 - phoneIn) * 8}deg)`,
            opacity: Math.min(1, phoneIn * 1.4) * (1 - phoneOut),
          }}
        >
          <div style={{ zoom: PZ }}>
            <Phone t={s} pageAt={3.05} tap={tap} />
          </div>
          {finger > 0 && (
            <div
              style={{
                position: "absolute",
                left: 300 * PZ * 0.5 - 30,
                top: 237 * PZ - 30,
                width: 60,
                height: 60,
                borderRadius: "50%",
                background: "rgba(255,255,255,0.28)",
                border: "2px solid rgba(255,255,255,0.65)",
                opacity: finger,
                transform: `scale(${1 - 0.15 * Math.sin(Math.min(1, Math.max(0, (s - TAP) / 0.2)) * Math.PI)})`,
              }}
            />
          )}
        </div>
      )}
      <div
        style={{
          position: "absolute",
          left: SHARE_C.x,
          top: top + PANEL_H * Z + 34,
          transform: "translateX(-50%)",
          ...reveal(s, DL + 0.05, fold - 0.15, { dy: 12, blur: 6, dur: 0.45 }),
        }}
      >
        <div style={{ zoom: 1.6 }}>
          <Toast>有人下载了「产品设计稿 v2.zip」· 192.168.1.41</Toast>
        </div>
      </div>
    </AbsoluteFill>
  );
}
