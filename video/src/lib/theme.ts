// The app's dark-theme tokens (src/styles.css, [data-theme="dark"]) plus the
// brand gradient, so the recreated panels look exactly like the real ones.
export const C = {
  // video backdrop: a touch darker than the app's own background
  void: "#04070a",
  bg: "#10151d",
  panel: "#171e28",
  sidebar: "#0c1117",
  ink: "#c6d0da",
  ink2: "#e2eaf1",
  muted: "#66727f",
  muted2: "#8b98a5",
  border: "rgba(255, 255, 255, 0.06)",
  border2: "rgba(255, 255, 255, 0.12)",
  accent: "#7fd4d4",
  accentInk: "#93dede",
  accentSoft: "rgba(127, 212, 212, 0.12)",
  accentFg: "#0d1319",
  success: "#7fbf93",
  successSoft: "rgba(127, 191, 147, 0.15)",
  danger: "#cf8a80",
  dangerSoft: "rgba(207, 138, 128, 0.14)",
  catImg: "#a8a6d4",
  catImgSoft: "rgba(168, 166, 212, 0.14)",
  catDoc: "#cbaa84",
  catDocSoft: "rgba(203, 170, 132, 0.15)",
  ringA: "rgba(127, 212, 212, 0.14)",
  ringB: "rgba(127, 212, 212, 0.09)",
  ringC: "rgba(127, 212, 212, 0.055)",
  sweep: "rgba(127, 212, 212, 0.13)",
  radial: "rgba(127, 212, 212, 0.05)",
  glow: "rgba(127, 212, 212, 0.6)",
  track: "rgba(255, 255, 255, 0.08)",
  shadow: "0 16px 48px rgba(0, 0, 0, 0.55)",
  toastBg: "#e2eaf1",
  toastFg: "#10151d",
  brandA: "#35ABC9",
  brandB: "#1F7A96",
} as const;

// Same stacks as the app: Space Grotesk for Latin, the system CJK face after it.
export const FONT =
  '"Space Grotesk Variable", "Space Grotesk", "PingFang SC", "Hiragino Sans GB", "Noto Sans SC", "Microsoft YaHei", system-ui, sans-serif';
export const MONO =
  '"IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, "PingFang SC", monospace';

export const W = 1920;
export const H = 1080;
