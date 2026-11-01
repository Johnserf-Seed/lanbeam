// The theme picks itself before the app has loaded (index.html) and switches
// from the header (nextThemeMode). Both have to agree with resolvedTheme, the
// rule AppShell applies once it is up.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { resolvedTheme, usePrefs, type ThemeMode } from "./store";
import { nextThemeMode } from "./theme";

describe("nextThemeMode — the header's switch", () => {
  it("flips what is on screen, and lands on 跟随系统 when that matches the system", () => {
    // On a light system: system → dark → back to following the system.
    expect(nextThemeMode("system", false)).toBe("dark");
    expect(nextThemeMode("dark", false)).toBe("system");
    // On a dark system, the same the other way round.
    expect(nextThemeMode("system", true)).toBe("light");
    expect(nextThemeMode("light", true)).toBe("system");
    // A theme pinned to the system's own still flips what you see.
    expect(nextThemeMode("light", false)).toBe("dark");
    expect(nextThemeMode("dark", true)).toBe("light");
  });

  it("always changes what is on screen", () => {
    for (const mode of ["light", "dark", "system"] as ThemeMode[])
      for (const sysDark of [false, true])
        expect(resolvedTheme(nextThemeMode(mode, sysDark), sysDark)).not.toBe(
          resolvedTheme(mode, sysDark),
        );
  });
});

describe("index.html's pre-paint theme", () => {
  const html = readFileSync(resolve(process.cwd(), "index.html"), "utf8");
  const script = html.match(/<script>([\s\S]*?)<\/script>/)?.[1] ?? "";

  /** Run the page's script against `saved` prefs and the system's scheme. */
  const paint = (saved: string | null, sysDark: boolean) => {
    localStorage.clear();
    if (saved !== null) localStorage.setItem("lanbeam.prefs", saved);
    vi.stubGlobal(
      "matchMedia",
      (q: string) =>
        ({ matches: sysDark && q.includes("dark") }) as MediaQueryList,
    );
    document.documentElement.removeAttribute("data-theme");
    new Function(script)();
    return document.documentElement.getAttribute("data-theme");
  };
  const prefs = (themeMode: unknown) =>
    JSON.stringify({ state: { themeMode }, version: 0 });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it("is there to run before the app does", () => {
    expect(script).toContain("lanbeam.prefs");
    expect(html.indexOf("<script>")).toBeLessThan(html.indexOf("</head>"));
  });

  it("resolves a saved choice exactly as the app will", () => {
    for (const mode of ["light", "dark", "system"] as ThemeMode[])
      for (const sysDark of [false, true])
        expect(paint(prefs(mode), sysDark), `${mode}/${sysDark}`).toBe(
          resolvedTheme(mode, sysDark),
        );
  });

  it("with nothing saved — or something unreadable — uses the store's default", () => {
    const fallback = usePrefs.getInitialState().themeMode;
    expect(fallback).toBe("system");
    for (const saved of [null, "{not json", prefs("sepia"), "null"])
      for (const sysDark of [false, true])
        expect(paint(saved, sysDark), `${saved}/${sysDark}`).toBe(
          resolvedTheme(fallback, sysDark),
        );
  });
});
