import type { CSSProperties, ReactNode } from "react";
import { C, FONT } from "../lib/theme";

/** "● 本机 · 书房 · MacBook Pro" — which screen we are looking at. */
export function DeviceLabel({
  children,
  style,
  dot = C.accent,
}: {
  children: ReactNode;
  style?: CSSProperties;
  dot?: string;
}) {
  return (
    <div
      style={{
        position: "absolute",
        display: "flex",
        alignItems: "center",
        gap: 12,
        fontFamily: FONT,
        fontSize: 22,
        fontWeight: 500,
        letterSpacing: "0.06em",
        color: C.muted2,
        whiteSpace: "nowrap",
        ...style,
      }}
    >
      <span
        style={{
          width: 9,
          height: 9,
          borderRadius: "50%",
          background: dot,
          boxShadow: `0 0 10px ${dot}`,
          flex: "none",
        }}
      />
      {children}
    </div>
  );
}
