// Small pieces of the app's UI, recreated from src/components/ui.tsx and
// src/styles.css (dark theme) at the app's own CSS sizes. Panels are drawn at
// 1× and enlarged with CSS `zoom`, so text stays sharp.
import type { CSSProperties, ReactNode } from "react";
import { C, FONT, MONO } from "../lib/theme";

const CAT: Record<string, [string, string]> = {
  ZIP: [C.accentInk, C.accentSoft],
  KEY: [C.catDoc, C.catDocSoft],
  HEIC: [C.catImg, C.catImgSoft],
  PNG: [C.catImg, C.catImgSoft],
};

export function ExtChip({
  ext,
  size = 32,
  fontSize = 9,
  radius,
  txt,
}: {
  ext: string;
  size?: number;
  fontSize?: number;
  radius?: number;
  txt?: boolean;
}) {
  const [fg, bg] = txt ? [C.accentInk, C.accentSoft] : (CAT[ext] ?? [C.muted2, C.sidebar]);
  const glyph = Math.round(size * 0.5);
  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: radius ?? Math.round(size / 4),
        background: bg,
        color: fg,
        fontSize,
        fontFamily: MONO,
        fontWeight: 600,
        display: "grid",
        placeItems: "center",
        flex: "none",
      }}
    >
      {txt ? (
        <svg width={glyph} height={glyph} viewBox="0 0 16 16" fill="currentColor">
          <rect x="3" y="3.6" width="10" height="1.7" rx="0.85" />
          <rect x="3" y="7.15" width="10" height="1.7" rx="0.85" />
          <rect x="3" y="10.7" width="6.4" height="1.7" rx="0.85" />
        </svg>
      ) : (
        ext
      )}
    </span>
  );
}

export function DirBadge({ style }: { style?: CSSProperties }) {
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        fontFamily: MONO,
        fontSize: 10,
        fontWeight: 700,
        borderRadius: 99,
        padding: "2px 8px",
        whiteSpace: "nowrap",
        color: C.accentInk,
        background: C.accentSoft,
        ...style,
      }}
    >
      ↑ 发送
    </span>
  );
}

/** `.btn` variants */
export function Btn({
  children,
  kind = "plain",
  style,
  press = 0,
}: {
  children: ReactNode;
  kind?: "plain" | "primary" | "danger" | "ghost";
  style?: CSSProperties;
  /** 0..1 pressed-in amount */
  press?: number;
}) {
  const base: CSSProperties = {
    height: 32,
    padding: "0 13px",
    borderRadius: 8,
    border: `1px solid ${C.border2}`,
    background: C.panel,
    color: C.muted2,
    fontSize: 12,
    fontWeight: 600,
    whiteSpace: "nowrap",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: 7,
    fontFamily: FONT,
    transform: `scale(${1 - press * 0.06})`,
  };
  if (kind === "primary")
    Object.assign(base, {
      border: "none",
      background: C.accent,
      color: C.accentFg,
      fontSize: 12.5,
      padding: "0 15px",
      filter: press ? `brightness(${1 - press * 0.08})` : undefined,
      boxShadow: press ? `0 0 0 ${press * 4}px rgba(127,212,212,0.18)` : undefined,
    });
  if (kind === "danger") base.color = C.danger;
  if (kind === "ghost") Object.assign(base, { border: "none", background: "none" });
  return <div style={{ ...base, ...style }}>{children}</div>;
}

export function Toggle({ on, size = "md" }: { on: number; size?: "md" | "sm" }) {
  const w = size === "sm" ? 32 : 36;
  const h = size === "sm" ? 18 : 20;
  const k = size === "sm" ? 14 : 16;
  return (
    <div
      style={{
        width: w,
        height: h,
        borderRadius: 99,
        background: on > 0.5 ? C.accent : C.track,
        position: "relative",
        flex: "none",
      }}
    >
      <div
        style={{
          position: "absolute",
          top: 2,
          left: 2 + on * (w - k - 4),
          width: k,
          height: k,
          borderRadius: "50%",
          background: "#fff",
          boxShadow: "0 1px 3px rgba(0,0,0,0.25)",
        }}
      />
    </div>
  );
}

/** `.modal` with the app's ModalHead and `.modal-foot`. */
export function Modal({
  width,
  title,
  sub,
  foot,
  children,
  style,
}: {
  width: number;
  title: ReactNode;
  sub?: ReactNode;
  foot?: ReactNode;
  children: ReactNode;
  style?: CSSProperties;
}) {
  return (
    <div
      style={{
        width,
        background: C.panel,
        border: `1px solid ${C.border}`,
        borderRadius: 16,
        boxShadow: C.shadow,
        overflow: "hidden",
        color: C.ink,
        fontFamily: FONT,
        fontSize: 13,
        lineHeight: 1.5,
        ...style,
      }}
    >
      <div style={{ display: "flex", alignItems: "flex-start", gap: 10, padding: "18px 20px 0" }}>
        <div style={{ flex: 1 }}>
          <div style={{ fontSize: 15, fontWeight: 650, color: C.ink2 }}>{title}</div>
          {sub != null && (
            <div style={{ fontSize: 11.5, color: C.muted, marginTop: 2 }}>{sub}</div>
          )}
        </div>
        <div
          style={{
            width: 28,
            height: 28,
            display: "grid",
            placeItems: "center",
            color: C.muted,
            fontSize: 15,
          }}
        >
          ×
        </div>
      </div>
      {children}
      {foot != null && (
        <div
          style={{
            background: C.sidebar,
            padding: "10px 20px",
            fontSize: 10.5,
            color: C.muted2,
          }}
        >
          {foot}
        </div>
      )}
    </div>
  );
}

/** The app's toast pill (dark theme: light pill, dark text). */
export function Toast({ children, style }: { children: ReactNode; style?: CSSProperties }) {
  return (
    <div
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 14,
        background: C.toastBg,
        color: C.toastFg,
        fontFamily: FONT,
        fontSize: 12.5,
        fontWeight: 600,
        padding: "9px 17px",
        borderRadius: 18,
        boxShadow: C.shadow,
        whiteSpace: "nowrap",
        ...style,
      }}
    >
      {children}
    </div>
  );
}
