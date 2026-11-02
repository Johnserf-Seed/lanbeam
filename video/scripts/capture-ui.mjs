#!/usr/bin/env node
// Capture the real LanBeam window from the browser demo (`?demo=1`) for the
// promo video's "look" scene. Run from the repo root after `pnpm install` there:
//
//   node video/scripts/capture-ui.mjs                 # → video/public/ui/*.webp
//   node video/scripts/capture-ui.mjs --out /tmp/ui
//
// Same approach as scripts/screenshots.mjs: Vite serves the app, a locally
// installed Chrome / Edge renders it through playwright-core (set CHROME_PATH
// for another Chromium), and the scene is staged through the app's own zustand
// stores. Shots are the bare 1280×800 window at 2×, as WebP q0.92 — the video
// draws its own frame around them.
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";
import { createServer } from "vite";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const WINDOW = { width: 1280, height: 800 };
const SCALE = 2;
const QUALITY = 0.92;
const NOW = new Date("2026-10-05T15:30:00+08:00");

// The look scene wipes light → dark → English.
const SHOTS = [
  { lang: "zh", theme: "light" },
  { lang: "zh", theme: "dark" },
  { lang: "en", theme: "dark" },
];

const { values: opts } = parseArgs({ options: { out: { type: "string" } } });
const OUT = opts.out ?? join(root, "video", "public", "ui");

async function launchBrowser() {
  const tries = process.env.CHROME_PATH
    ? [{ executablePath: process.env.CHROME_PATH }]
    : [{ channel: "chrome" }, { channel: "msedge" }, {}];
  for (const t of tries) {
    try {
      return await chromium.launch({ ...t, headless: true });
    } catch {}
  }
  throw new Error("No Chromium found — install Chrome, or set CHROME_PATH.");
}

async function main() {
  const server = await createServer({
    root,
    logLevel: "warn",
    server: { port: 14_300, strictPort: false },
  });
  await server.listen();
  const base = server.resolvedUrls.local[0].replace(/\/$/, "");
  const browser = await launchBrowser();
  await mkdir(OUT, { recursive: true });
  try {
    for (const { lang, theme } of SHOTS) {
      const ctx = await browser.newContext({
        viewport: WINDOW,
        deviceScaleFactor: SCALE,
        colorScheme: theme,
        locale: lang === "zh" ? "zh-CN" : "en-US",
        timezoneId: "Asia/Shanghai",
      });
      await ctx.clock.setFixedTime(NOW);
      await ctx.addInitScript(
        ({ lang, theme }) => {
          localStorage.setItem("lanbeam.lang", lang);
          localStorage.setItem(
            "lanbeam.prefs",
            JSON.stringify({ state: { themeMode: theme }, version: 0 }),
          );
        },
        { lang, theme },
      );
      const page = await ctx.newPage();
      await page.goto(`${base}/?demo=1#/`, { waitUntil: "networkidle" });
      await page.evaluate(() => document.fonts.ready);
      await page.waitForTimeout(500);
      // Age every peer but one, like a radar that has been open a while.
      await page.evaluate(async () => {
        const S = await import("/src/lib/store/index.ts");
        const old = Date.now() - 10 * 60_000;
        const firstSeen = { ...S.useData.getState().firstSeen };
        for (const id of Object.keys(firstSeen))
          if (id !== "demo-ipad") firstSeen[id] = old;
        S.useData.setState({ firstSeen });
      });
      await page.waitForTimeout(900);
      const png = await page.screenshot({ type: "png" });
      const webp = await page.evaluate(
        async ({ src, q }) => {
          const img = new Image();
          img.src = src;
          await img.decode();
          const cv = document.createElement("canvas");
          cv.width = img.width;
          cv.height = img.height;
          cv.getContext("2d").drawImage(img, 0, 0);
          return cv.toDataURL("image/webp", q);
        },
        { src: `data:image/png;base64,${png.toString("base64")}`, q: QUALITY },
      );
      const file = join(OUT, `devices-${lang}-${theme}.webp`);
      await writeFile(file, Buffer.from(webp.split(",")[1], "base64"));
      console.log(`  ${file.slice(root.length + 1)}`);
      await ctx.close();
    }
  } finally {
    await browser.close();
    await server.close();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
