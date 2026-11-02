// The running-transfer row of the 传输 page (src/pages/TransfersPage.tsx),
// inside its「进行中」card.
import type { CSSProperties, ReactNode } from "react";
import { C, FONT, MONO } from "../lib/theme";
import { DirBadge, ExtChip } from "./primitives";

export function TransferCard({
  pct,
  speed,
  eta,
  status,
  paused,
  hist,
  pausePress = 0,
  done = 0,
  width = 760,
}: {
  pct: number;
  speed: number;
  eta: string;
  status: ReactNode;
  paused: boolean;
  /** speed samples, newest last */
  hist: number[];
  pausePress?: number;
  /** 0..1 completion glow */
  done?: number;
  width?: number;
}) {
  const doneMb = (1229 * pct) / 100;
  const doneStr =
    doneMb <= 0 ? "0 KB" : doneMb >= 1000 ? `${(doneMb / 1024).toFixed(1)} GB` : `${doneMb >= 10 ? Math.round(doneMb) : doneMb.toFixed(1)} MB`;
  const mx = Math.max(20, ...hist);
  const tick = (at: number): CSSProperties => ({
    position: "absolute",
    left: `${at}%`,
    top: 0,
    bottom: 0,
    width: 1,
    background: pct > at ? "rgba(255,255,255,.55)" : C.border2,
  });
  return (
    <div
      style={{
        width,
        background: C.panel,
        border: `1px solid ${C.border}`,
        borderRadius: 12,
        overflow: "hidden",
        boxShadow: C.shadow,
        fontFamily: FONT,
        color: C.ink,
        fontSize: 13,
        lineHeight: 1.5,
      }}
    >
      <div style={{ padding: "11px 16px", borderBottom: `1px solid ${C.border}`, display: "flex", alignItems: "baseline", gap: 8 }}>
        <span style={{ fontSize: 12.5, fontWeight: 650, color: C.ink2 }}>进行中</span>
        <span style={{ fontSize: 11, color: C.muted }}>1 项</span>
      </div>
      <div style={{ padding: "13px 16px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <ExtChip ext="ZIP" size={32} fontSize={9} radius={8} />
          <div style={{ flex: 1, minWidth: 0, whiteSpace: "nowrap" }}>
            <span style={{ fontSize: 12.5, fontWeight: 600, color: C.ink2 }}>产品设计稿 v2.zip</span>
            <DirBadge style={{ marginLeft: 8, verticalAlign: 1 }} />
            <span style={{ fontSize: 11, color: C.muted, marginLeft: 6 }}>客厅 · Mac mini · 1.2 GB</span>
          </div>
          <div style={{ textAlign: "right", flex: "none" }}>
            <div style={{ fontFamily: MONO, fontSize: 11.5, color: paused ? C.muted : C.accentInk }}>
              {speed.toFixed(1)} MB/s
            </div>
            <div style={{ fontSize: 10.5, color: C.muted }}>{eta}</div>
          </div>
          <div
            style={{
              fontFamily: MONO,
              fontSize: 11.5,
              fontWeight: 600,
              color: done > 0.5 ? C.success : C.ink2,
              width: 44,
              textAlign: "right",
              flex: "none",
            }}
          >
            {Math.round(pct)}%
          </div>
          <div style={{ display: "flex", gap: 4, flex: "none" }}>
            <div
              style={{
                padding: "4px 10px",
                borderRadius: 7,
                fontSize: 11.5,
                color: C.muted2,
                border: `1px solid ${C.border2}`,
                background: pausePress > 0 ? "rgba(255,255,255,0.06)" : C.panel,
                transform: `scale(${1 - pausePress * 0.07})`,
                whiteSpace: "nowrap",
              }}
            >
              {paused ? "继续" : "暂停"}
            </div>
            <div style={{ padding: "4px 10px", fontSize: 11.5, color: C.muted, whiteSpace: "nowrap" }}>取消</div>
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "flex-end", gap: 14, margin: "9px 0 0 44px" }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ position: "relative", height: 5, borderRadius: 99, background: C.track, overflow: "hidden" }}>
              <div
                style={{
                  width: `${pct}%`,
                  height: "100%",
                  borderRadius: 99,
                  background: done > 0 ? `color-mix(in srgb, ${C.success} ${done * 100}%, ${C.accent})` : C.accent,
                  boxShadow: paused ? undefined : `0 0 10px ${C.glow}`,
                }}
              />
              <div style={tick(25)} />
              <div style={tick(50)} />
              <div style={tick(75)} />
            </div>
            <div style={{ fontFamily: MONO, fontSize: 10.5, color: C.muted, marginTop: 6, whiteSpace: "nowrap" }}>
              {doneStr} / 1.2 GB · {Math.round(pct)}% · {status}
            </div>
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "flex-end",
              gap: 1,
              height: 30,
              width: 162,
              flex: "none",
              borderBottom: `1px solid ${C.border2}`,
              paddingBottom: 1,
              overflow: "hidden",
            }}
          >
            {hist.slice(-40).map((v, i, a) => (
              <div
                // biome-ignore lint/suspicious/noArrayIndexKey: positional bars
                key={i}
                style={{
                  width: 3,
                  height: 2 + (v / mx) * 26,
                  background: C.accent,
                  opacity: i === a.length - 1 ? 1 : 0.85,
                  borderRadius: "1px 1px 0 0",
                  flex: "none",
                }}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
