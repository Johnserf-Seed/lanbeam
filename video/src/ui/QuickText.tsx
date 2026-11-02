// 快传文本 (src/components/QuickTextModal.tsx) and the receiver's inbox row
// for a text that went straight to the clipboard (src/pages/InboxPage.tsx).
import { C, FONT, MONO } from "../lib/theme";
import { ExtChip, Modal, Toggle } from "./primitives";

export function QuickTextPanel({
  text,
  caret,
  bytesLabel,
  clip,
  sendPress = 0,
  textLift = 0,
}: {
  text: string;
  caret: boolean;
  bytesLabel: string;
  clip: number;
  sendPress?: number;
  /** 0..1 the text leaving the box */
  textLift?: number;
}) {
  return (
    <Modal width={440} title="⌁ 快传文本" sub="发一段文字或链接过去 · 不落盘" foot="端到端加密 · 对方在线即达 · 也可用全局快捷键唤起">
      <div style={{ padding: "14px 20px 18px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span style={{ fontSize: 11.5, color: C.muted2, flex: "none" }}>发送到</span>
          <div
            style={{
              flex: 1,
              height: 30,
              borderRadius: 8,
              border: `1px solid ${C.border2}`,
              background: C.panel,
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              padding: "0 9px 0 10px",
              fontSize: 12,
              color: C.ink,
            }}
          >
            客厅 · Mac mini
            <svg width="10" height="6" viewBox="0 0 10 6">
              <path d="M1 1l4 4 4-4" fill="none" stroke="#8b98a5" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </div>
        </div>
        <div
          style={{
            marginTop: 12,
            height: 96,
            borderRadius: 10,
            border: `1px solid ${C.border2}`,
            background: C.bg,
            padding: "10px 12px",
            fontFamily: MONO,
            fontSize: 12,
            lineHeight: 1.5,
            color: C.ink2,
            whiteSpace: "pre-wrap",
            position: "relative",
            overflow: "hidden",
          }}
        >
          <span
            style={{
              display: "inline",
              opacity: 1 - textLift,
              filter: textLift ? `blur(${textLift * 6}px)` : undefined,
            }}
          >
            {text}
          </span>
          {caret && (
            <span
              style={{
                display: "inline-block",
                width: 1.5,
                height: 15,
                background: C.accent,
                verticalAlign: -3,
                marginLeft: 1,
              }}
            />
          )}
        </div>
        <div style={{ textAlign: "right", fontSize: 10.5, color: C.muted, marginTop: 6 }}>{bytesLabel}</div>
        <div style={{ display: "flex", alignItems: "center", gap: 9, marginTop: 10 }}>
          <Toggle on={clip} />
          <span style={{ fontSize: 11.5, color: C.muted2, flex: 1 }}>请求同时写入对方剪贴板</span>
          <div
            style={{
              height: 32,
              padding: "0 15px",
              borderRadius: 8,
              background: C.accent,
              color: C.accentFg,
              fontSize: 12.5,
              fontWeight: 600,
              display: "flex",
              alignItems: "center",
              gap: 8,
              transform: `scale(${1 - sendPress * 0.07})`,
              boxShadow: sendPress ? `0 0 0 ${sendPress * 4}px rgba(127,212,212,0.18)` : undefined,
            }}
          >
            发送 <span style={{ fontSize: 11, opacity: 0.85 }}>⌘↩</span>
          </div>
        </div>
      </div>
    </Modal>
  );
}

export function InboxTextRow({ badge }: { badge: number }) {
  return (
    <div
      style={{
        width: 420,
        background: C.panel,
        border: `1px solid ${C.border}`,
        borderRadius: 12,
        boxShadow: C.shadow,
        fontFamily: FONT,
        color: C.ink,
        overflow: "hidden",
      }}
    >
      <div style={{ padding: "9px 14px", borderBottom: `1px solid ${C.border}`, fontSize: 11, color: C.muted, display: "flex", justifyContent: "space-between" }}>
        <span style={{ fontWeight: 650, color: C.ink2, fontSize: 12 }}>收件箱</span>
        <span>今天</span>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 11, padding: "11px 14px" }}>
        <ExtChip ext="TXT" txt size={30} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 12.5, fontWeight: 600, color: C.ink2, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
            https://figma.com/file/lb-review 评审改到周四 10:30…
          </div>
          <div style={{ fontSize: 10.5, color: C.muted, marginTop: 1 }}>来自 书房 · MacBook Pro · 15:31 · 1 KB</div>
        </div>
        <span
          style={{
            fontSize: 10,
            fontWeight: 600,
            color: C.success,
            background: C.successSoft,
            borderRadius: 99,
            padding: "2px 8px",
            flex: "none",
            opacity: badge,
            transform: `scale(${0.8 + 0.2 * badge})`,
          }}
        >
          已入剪贴板
        </span>
      </div>
    </div>
  );
}
