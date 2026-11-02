// 用浏览器接收 (src/components/ShareModal.tsx + Qr.tsx) and the page the phone
// opens (src-tauri/src/share.rs: render_landing + SHARE_CSS, dark scheme).
import { create } from "qrcode";
import { useMemo, type ReactNode } from "react";
import { E, clamp01, prog } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";
import { BrandMark } from "../components/BrandMark";
import { Modal } from "./primitives";

export const SHARE_URL = "http://192.168.1.20:51705/s/n2gj5ezpnea0q4xk3stp7zn";

/** The real, scannable code (level M, 4-module quiet zone, like the app),
 *  with each module popping in along a diagonal sweep. */
export function Qr({ value, size, t, at, dur = 0.9 }: { value: string; size: number; t: number; at: number; dur?: number }) {
  const m = useMemo(() => {
    const { modules } = create(value, { errorCorrectionLevel: "M" });
    const n = modules.size;
    const cells: { x: number; y: number }[] = [];
    for (let r = 0; r < n; r++) for (let c = 0; c < n; c++) if (modules.get(r, c)) cells.push({ x: c, y: r });
    return { n, cells };
  }, [value]);
  const dim = m.n + 8;
  return (
    <svg width={size} height={size} viewBox={`0 0 ${dim} ${dim}`} shapeRendering="crispEdges" style={{ display: "block" }}>
      <rect width={dim} height={dim} fill="#fff" rx={2.2} />
      {m.cells.map((c) => {
        const d = (c.x + c.y) / (2 * m.n);
        const p = prog(t, at + d * dur, at + d * dur + 0.25, E.out);
        if (p <= 0) return null;
        const s = 0.4 + 0.6 * p;
        return (
          <rect
            key={`${c.x}-${c.y}`}
            x={4 + c.x + (1 - s) / 2}
            y={4 + c.y + (1 - s) / 2}
            width={s + 0.02}
            height={s + 0.02}
            fill="#10151d"
            opacity={p}
          />
        );
      })}
    </svg>
  );
}

export function SharePanel({
  t,
  linkChars,
  qrAt,
  downloads,
  toast,
}: {
  t: number;
  linkChars: number;
  qrAt: number;
  downloads: ReactNode;
  toast?: ReactNode;
}) {
  const shown = SHARE_URL.slice(0, linkChars);
  const chip = (label: string) => (
    <span
      style={{
        fontSize: 11,
        fontWeight: 600,
        color: C.accentInk,
        background: C.accentSoft,
        borderRadius: 99,
        padding: "4px 11px",
        whiteSpace: "nowrap",
      }}
    >
      {label} <span style={{ fontSize: 8 }}>▾</span>
    </span>
  );
  return (
    <Modal width={440} title="用浏览器接收" sub="对方无需安装 · 将分享 3 个文件 · 1.3 GB" foot="仅局域网可访问 · 不经外网 · 停止分享后链接立即失效">
      <div style={{ padding: "14px 20px 16px", position: "relative" }}>
        <div style={{ display: "flex", gap: 12, alignItems: "stretch" }}>
          <div
            style={{
              flex: 1,
              minWidth: 0,
              background: C.sidebar,
              borderRadius: 10,
              padding: "10px 13px",
              display: "flex",
              flexDirection: "column",
              justifyContent: "space-between",
            }}
          >
            <div style={{ fontFamily: MONO, fontSize: 12, fontWeight: 600, color: C.accentInk, lineHeight: 1.6, wordBreak: "break-all", minHeight: 38 }}>
              {shown}
              {linkChars < SHARE_URL.length && (
                <span style={{ display: "inline-block", width: 7, height: 13, background: C.accent, verticalAlign: -2, marginLeft: 1, opacity: 0.8 }} />
              )}
            </div>
            <div style={{ fontSize: 11, fontWeight: 600, color: C.accentInk, marginTop: 6 }}>复制链接</div>
          </div>
          <div style={{ flex: "none", padding: 0, borderRadius: 10, overflow: "hidden" }}>
            <Qr value={SHARE_URL} size={84} t={t} at={qrAt} />
          </div>
        </div>
        <div
          style={{
            marginTop: 10,
            background: C.sidebar,
            borderRadius: 10,
            padding: "9px 11px",
            fontSize: 11,
            color: C.muted2,
            lineHeight: 1.65,
          }}
        >
          局域网直连 · 文件从本机直达对方，不经过任何服务器。若浏览器提示「不安全」，是局域网链接的正常现象，点「继续」即可打开。
        </div>
        <div style={{ fontSize: 11.5, color: C.muted, marginTop: 9 }}>{downloads}</div>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 9 }}>
          {chip("有效 10 分钟")}
          {chip("每个文件 1 次")}
          <span style={{ flex: 1 }} />
          <span
            style={{
              fontSize: 11,
              fontWeight: 600,
              color: C.danger,
              border: `1px solid ${C.border2}`,
              borderRadius: 7,
              padding: "3px 12px",
            }}
          >
            停止分享
          </span>
        </div>
        {toast}
      </div>
    </Modal>
  );
}

const FILES = [
  { ext: "ZIP", name: "产品设计稿 v2.zip", size: "1.2 GB" },
  { ext: "KEY", name: "季度汇报.key", size: "86 MB" },
  { ext: "HEIC", name: "IMG_0231.HEIC", size: "4.6 MB" },
];

/** A phone showing the landing page. `tap` (0..1) ripples the first row. */
export function Phone({ t, pageAt, tap = -1 }: { t: number; pageAt: number; tap?: number }) {
  const page = prog(t, pageAt, pageAt + 0.6, E.out);
  return (
    <div
      style={{
        width: 300,
        height: 620,
        borderRadius: 46,
        padding: 9,
        background: "linear-gradient(160deg, #2a323d 0%, #12171e 45%, #0b0f14 100%)",
        boxShadow:
          "0 0 0 1px rgba(255,255,255,0.10), 0 40px 90px rgba(0,0,0,0.65), inset 0 0 0 1.5px rgba(255,255,255,0.06)",
        position: "relative",
      }}
    >
      <div
        style={{
          width: "100%",
          height: "100%",
          borderRadius: 38,
          overflow: "hidden",
          position: "relative",
          background: `radial-gradient(130% 90% at 50% -20%, ${C.accentSoft}, transparent 55%), ${C.bg}`,
          fontFamily: '-apple-system, "PingFang SC", system-ui, sans-serif',
          color: C.ink2,
        }}
      >
        {/* status bar + island */}
        <div style={{ height: 40, display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0 26px", fontSize: 12, fontWeight: 600, color: C.ink2 }}>
          <span>15:30</span>
          <span style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <span style={{ width: 15, height: 9, borderRadius: 2, border: "1px solid rgba(226,234,241,0.7)", position: "relative" }}>
              <span style={{ position: "absolute", inset: 1.5, right: 4, background: C.ink2, borderRadius: 1 }} />
            </span>
          </span>
        </div>
        <div style={{ position: "absolute", top: 11, left: "50%", marginLeft: -44, width: 88, height: 24, borderRadius: 14, background: "#000" }} />
        {/* address bar */}
        <div style={{ margin: "2px 14px 0", height: 30, borderRadius: 10, background: "rgba(255,255,255,0.07)", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 11, color: C.muted2, gap: 6 }}>
          <span style={{ fontFamily: MONO, fontSize: 10.5 }}>192.168.1.20:51705</span>
        </div>
        <div style={{ height: 2, margin: "6px 14px 0", borderRadius: 2, background: "rgba(255,255,255,0.06)", overflow: "hidden" }}>
          <div style={{ width: `${clamp01(page * 1.4) * 100}%`, height: "100%", background: C.accent, opacity: page < 1 ? 1 : 0 }} />
        </div>
        {/* the landing page */}
        <div
          style={{
            padding: "22px 14px",
            opacity: page,
            transform: `translateY(${(1 - page) * 12}px)`,
          }}
        >
          <div
            style={{
              background: C.panel,
              border: "1px solid rgba(255,255,255,.09)",
              borderRadius: 20,
              boxShadow: "0 20px 60px rgba(0,0,0,.45)",
              padding: "22px 16px 18px",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 11, marginBottom: 16 }}>
              <div style={{ filter: "drop-shadow(0 5px 14px rgba(31,122,150,.35))" }}>
                <BrandMark size={36} glow={0} />
              </div>
              <div>
                <div style={{ fontSize: 18, fontWeight: 700, letterSpacing: "-.01em", fontFamily: FONT }}>LanBeam</div>
                <div style={{ fontSize: 11, color: C.muted }}>通过局域网分享</div>
              </div>
            </div>
            <div style={{ fontSize: 12.5, color: C.muted2, marginBottom: 13 }}>向你分享了 3 个文件 — 点按下载。</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {FILES.map((f, i) => {
                const rp = prog(t, pageAt + 0.25 + i * 0.1, pageAt + 0.85 + i * 0.1, E.out);
                const hot = i === 0 && tap >= 0;
                return (
                  <div
                    key={f.name}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 11,
                      padding: "10px 11px",
                      border: `1px solid ${hot ? C.accent : "rgba(255,255,255,.06)"}`,
                      borderRadius: 13,
                      background: C.bg,
                      opacity: rp,
                      transform: `translateY(${(1 - rp) * 10}px)`,
                      position: "relative",
                      overflow: "hidden",
                      boxShadow: hot ? "0 8px 22px rgba(75,139,163,.16)" : undefined,
                    }}
                  >
                    {hot && (
                      <div
                        style={{
                          position: "absolute",
                          left: 150,
                          top: 25,
                          width: 360 * tap,
                          height: 360 * tap,
                          marginLeft: -180 * tap,
                          marginTop: -180 * tap,
                          borderRadius: "50%",
                          background: "rgba(127,212,212,0.16)",
                          opacity: 1 - tap,
                        }}
                      />
                    )}
                    <span
                      style={{
                        width: 36,
                        height: 36,
                        flex: "none",
                        borderRadius: 10,
                        display: "grid",
                        placeItems: "center",
                        fontSize: 9,
                        fontWeight: 700,
                        background: C.accentSoft,
                        color: C.accentInk,
                      }}
                    >
                      {f.ext}
                    </span>
                    <span style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontSize: 12.5, fontWeight: 600, whiteSpace: "nowrap" }}>{f.name}</div>
                      <div style={{ fontSize: 10.5, color: C.muted, marginTop: 1 }}>{f.size}</div>
                    </span>
                    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" style={{ color: C.accent, opacity: hot ? 1 : 0.65, flex: "none" }}>
                      <path d="M12 3v11m0 0-4-4m4 4 4-4M5 21h14" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
                    </svg>
                  </div>
                );
              })}
            </div>
          </div>
          <div style={{ fontSize: 10, color: C.muted, textAlign: "center", lineHeight: 1.6, marginTop: 14, padding: "0 10px" }}>
            由发送方设备通过局域网直接提供 — 无账号 · 无云端 · 无第三方服务器。
          </div>
        </div>
      </div>
    </div>
  );
}
