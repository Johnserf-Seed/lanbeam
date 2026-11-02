// The receiver's side of a send: the incoming card
// (src/components/IncomingStack.tsx).
import type { ReactNode } from "react";
import { C, FONT, MONO } from "../lib/theme";
import { Btn } from "./primitives";

export function IncomingCard({
  peer,
  sub,
  file,
  sas,
  sasGlow = 0,
  acceptPress = 0,
}: {
  peer: string;
  sub: string;
  file: string;
  sas: ReactNode;
  sasGlow?: number;
  acceptPress?: number;
}) {
  return (
    <div
      style={{
        width: 302,
        background: C.panel,
        border: `1px solid ${C.border}`,
        borderRadius: 14,
        padding: "14px 16px",
        boxShadow: C.shadow,
        color: C.ink,
        fontFamily: FONT,
        fontSize: 13,
        lineHeight: 1.5,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <div
          style={{
            width: 32,
            height: 32,
            borderRadius: "50%",
            background: C.accentSoft,
            color: C.accentInk,
            display: "grid",
            placeItems: "center",
            fontSize: 13,
            fontWeight: 600,
            flex: "none",
          }}
        >
          {peer.charAt(0)}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: C.ink2, whiteSpace: "nowrap" }}>{peer}</div>
          <div style={{ fontSize: 11, color: C.muted }}>{sub}</div>
        </div>
        <div style={{ color: C.muted2, fontSize: 15, width: 22, textAlign: "center" }}>–</div>
      </div>
      <div style={{ display: "flex", gap: 6, marginTop: 11 }}>
        <span
          style={{
            fontFamily: MONO,
            fontSize: 10.5,
            color: C.muted2,
            background: C.sidebar,
            border: `1px solid ${C.border}`,
            borderRadius: 6,
            padding: "2px 7px",
            whiteSpace: "nowrap",
          }}
        >
          {file}
        </span>
      </div>
      <div
        style={{
          marginTop: 11,
          background: C.accentSoft,
          borderRadius: 10,
          padding: "9px 12px",
          textAlign: "center",
          boxShadow: sasGlow
            ? `0 0 0 ${1.5 * sasGlow}px rgba(127,212,212,${0.7 * sasGlow}), 0 0 ${26 * sasGlow}px rgba(127,212,212,${0.35 * sasGlow})`
            : undefined,
        }}
      >
        <div style={{ fontSize: 10, color: C.muted2, letterSpacing: ".04em" }}>校验码 · 与发送方屏幕核对一致再接收</div>
        <div
          style={{
            fontFamily: MONO,
            fontSize: 16,
            fontWeight: 600,
            color: C.accentInk,
            marginTop: 4,
            letterSpacing: ".05em",
            display: "flex",
            justifyContent: "center",
          }}
        >
          {sas}
        </div>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 7, marginTop: 11, fontSize: 11.5, color: C.muted2 }}>
        <span
          style={{
            width: 13,
            height: 13,
            borderRadius: 3,
            border: `1px solid ${C.border2}`,
            background: C.bg,
            flex: "none",
          }}
        />
        以后自动接收此设备
      </div>
      <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
        <Btn style={{ flex: 1, height: 30 }}>拒绝</Btn>
        <Btn kind="primary" press={acceptPress} style={{ flex: 1, height: 30 }}>
          接收
        </Btn>
      </div>
    </div>
  );
}
