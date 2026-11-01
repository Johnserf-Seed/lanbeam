import { resolvedTheme, type ThemeMode } from "./store";

/** The header's one-click theme switch: flip what is on screen — and when that
 *  lands on the system's own theme, follow the system again instead of pinning
 *  it. It used to toggle between explicit light and dark only, so one click
 *  pinned the theme for good and only Settings could get back to 跟随系统. */
export function nextThemeMode(mode: ThemeMode, sysDark: boolean): ThemeMode {
  const flipped = resolvedTheme(mode, sysDark) === "light" ? "dark" : "light";
  return flipped === (sysDark ? "dark" : "light") ? "system" : flipped;
}
