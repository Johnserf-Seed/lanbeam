import type { CSSProperties, MouseEventHandler, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { catColors } from "../lib/filecat";

/** Pill switch. Sizes: default 36×20, sm 32×18, xs 30×17. */
export function Toggle({
  on,
  onClick,
  size,
  stop,
}: {
  on: boolean;
  onClick: () => void;
  size?: "sm" | "xs";
  stop?: boolean;
}) {
  const h: MouseEventHandler = (e) => {
    if (stop) e.stopPropagation();
    onClick();
  };
  return (
    <button
      type="button"
      className={`toggle${on ? " on" : ""}${size ? ` ${size}` : ""}`}
      onClick={h}
    />
  );
}

/** Segmented filter control. */
export function Segmented<K extends string>({
  options,
  value,
  onChange,
  itemStyle,
}: {
  options: { key: K; label: string }[];
  value: K;
  onChange: (k: K) => void;
  itemStyle?: CSSProperties;
}) {
  return (
    <div className="seg">
      {options.map((o) => (
        <button
          key={o.key}
          type="button"
          className={`seg-item${o.key === value ? " active" : ""}`}
          style={itemStyle}
          onClick={() => onChange(o.key)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Square file-extension chip tinted by category. `txt` renders a clean
 *  text-lines glyph (three rounded bars) instead of the extension. */
export function ExtChip({
  ext,
  size = 32,
  fontSize = 9,
  radius,
  isTxt,
}: {
  ext: string;
  size?: number;
  fontSize?: number;
  radius?: number;
  isTxt?: boolean;
}) {
  const [fg, bg] = isTxt
    ? ["var(--accent-ink)", "var(--accent-soft)"]
    : catColors(ext);
  const glyph = Math.round(size * 0.5);
  return (
    <span
      className="ext-chip"
      style={{
        width: size,
        height: size,
        borderRadius: radius ?? Math.round(size / 4),
        background: bg,
        color: fg,
        fontSize,
      }}
    >
      {isTxt ? (
        <svg
          width={glyph}
          height={glyph}
          viewBox="0 0 16 16"
          fill="currentColor"
          aria-hidden="true"
        >
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

/** ↑ 发送 / ↓ 接收 direction pill. */
export function DirBadge({
  dir,
  style,
}: {
  dir: "send" | "receive";
  style?: CSSProperties;
}) {
  const { t } = useTranslation();
  return (
    <span
      className={`dir-badge ${dir === "send" ? "out" : "in"}`}
      style={style}
    >
      {dir === "send" ? t("transfers.dirOut") : t("transfers.dirIn")}
    </span>
  );
}

/** Live/away status dot with optional glow. */
export function StatusDot({
  online,
  size = 8,
  glow = true,
}: {
  online: boolean;
  size?: number;
  glow?: boolean;
}) {
  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: "50%",
        flex: "none",
        boxSizing: "border-box",
        background: online ? "var(--dot-live)" : "transparent",
        border: online ? "none" : "1.5px solid var(--muted)",
        boxShadow: online && glow ? "0 0 12px var(--glow)" : "none",
      }}
    />
  );
}

/** Modal header row: title + sub + close ×. */
export function ModalHead({
  title,
  sub,
  onClose,
  pad = "18px 20px 0",
}: {
  title: ReactNode;
  sub?: ReactNode;
  onClose: () => void;
  pad?: string;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 10,
        padding: pad,
      }}
    >
      <div style={{ flex: 1 }}>
        <div style={{ fontSize: 15, fontWeight: 650, color: "var(--ink2)" }}>
          {title}
        </div>
        {sub != null && (
          <div style={{ fontSize: 11.5, color: "var(--muted)", marginTop: 2 }}>
            {sub}
          </div>
        )}
      </div>
      <button
        type="button"
        onClick={onClose}
        style={{
          width: 28,
          height: 28,
          borderRadius: 8,
          border: "none",
          background: "none",
          color: "var(--muted)",
          fontSize: 15,
          cursor: "pointer",
        }}
        onMouseEnter={(e) =>
          (e.currentTarget.style.background = "var(--hover)")
        }
        onMouseLeave={(e) => (e.currentTarget.style.background = "none")}
      >
        ×
      </button>
    </div>
  );
}
