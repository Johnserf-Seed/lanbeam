#!/usr/bin/env node
// Regenerate the README screenshots from the browser demo (`?demo=1`):
//
//   pnpm screenshots                  # every language × theme
//   pnpm screenshots --lang en        # one language
//   pnpm screenshots --only devices,pair
//
// Starts Vite, drives a locally installed Chrome / Edge through playwright-core
// (nothing is downloaded — point CHROME_PATH at any other Chromium), and writes
// docs/screenshots/<lang>/<shot>[-dark].webp at 2×, framed like the frameless app
// window: rounded corners and a soft shadow on a transparent background. WebP
// at q0.92 is indistinguishable from PNG here at an eighth of the size, and the
// clock and time zone are pinned, so re-running only changes what really changed.
//
// The demo content follows the UI language (src/lib/demoContent.ts). Scenes
// are set up through the app's own zustand stores, imported in the page from
// the same /src URLs the app loaded — so a scene is exactly what a user would
// see, without scripting clicks against labels that change with the language.
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";
import { createServer } from "vite";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(root, "docs", "screenshots");

// A little roomier than the 1120×740 default window, so the incoming-request
// card on the devices shot doesn't sit on top of a radar label.
const WINDOW = { width: 1280, height: 800 };
const SCALE = 2;
const QUALITY = 0.92;
const NOW = new Date("2026-10-05T15:30:00+08:00");
const TIME_ZONE = "Asia/Shanghai";
const MARGIN = 36; // CSS px of transparent room around the window for its shadow
const RADIUS = 12;

/** quiet: drop the demo's pending incoming request (it floats over every page).
 *  scene: what to open on top, run in the page against the app's stores.
 *  themes: the README shows the hero in both (a <picture> follows GitHub's
 *  theme); the gallery is light only, which keeps the repo small. */
const SHOTS = [
  { name: "devices", route: "/", themes: ["light", "dark"] },
  { name: "send", route: "/", quiet: true, scene: "send" },
  { name: "share", route: "/", quiet: true, scene: "share" },
  { name: "pair", route: "/", quiet: true, scene: "pair" },
  { name: "transfers", route: "/transfers", quiet: true },
  { name: "inbox", route: "/inbox", quiet: true },
  { name: "trusted", route: "/trusted", quiet: true },
  { name: "settings", route: "/settings", quiet: true },
];

const { values: opts } = parseArgs({
  options: {
    lang: { type: "string" },
    theme: { type: "string" },
    only: { type: "string" },
  },
});
const langs = opts.lang ? [opts.lang] : ["en", "zh"];
const only = opts.only ? new Set(opts.only.split(",")) : null;

async function launchBrowser() {
  const tries = process.env.CHROME_PATH
    ? [{ executablePath: process.env.CHROME_PATH }]
    : [{ channel: "chrome" }, { channel: "msedge" }, {}];
  const errors = [];
  for (const t of tries) {
    try {
      return await chromium.launch({ ...t, headless: true });
    } catch (e) {
      errors.push(e.message.split("\n")[0]);
    }
  }
  throw new Error(
    `No Chromium found — install Chrome, or set CHROME_PATH.\n  ${errors.join("\n  ")}`,
  );
}

/** Set up a scene inside the page. Runs in the BROWSER: it imports the app's own
 *  store modules, which resolve to the instances the app is already using. */
async function stage(page, shot) {
  await page.evaluate(
    async ({ quiet, scene }) => {
      const S = await import("/src/lib/store.ts");
      const { demoContent } = await import("/src/lib/demoContent.ts");
      const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
      const c = demoContent();

      // Every peer was discovered "just now" on a fresh load; age all but one,
      // the way a radar looks after the app has been open for a while.
      const old = Date.now() - 10 * 60_000;
      const firstSeen = { ...S.useData.getState().firstSeen };
      for (const id of Object.keys(firstSeen))
        if (id !== "demo-ipad") firstSeen[id] = old;
      S.useData.setState({ firstSeen });

      if (quiet) S.useTransfers.setState({ incomings: [] });

      const dir = "/Users/demo/Downloads";
      const files = [
        { name: c.designZip, ext: "ZIP", size: 1229 * 1048576 },
        { name: c.reviewKey, ext: "KEY", size: 86 * 1048576 },
        { name: "IMG_0231.HEIC", ext: "HEIC", size: 4.6 * 1048576 },
        { name: c.whiteboard, ext: "PNG", size: 2.1 * 1048576 },
      ].map((f) => ({ ...f, path: `${dir}/${f.name}` }));
      const ov = S.useOverlays.getState();
      switch (scene) {
        case "pair":
          ov.setPair(true);
          break;
        case "send":
          ov.openSend("demo-mini", files, files.slice(0, 2));
          break;
        case "share":
          // The share panel publishes the send flow's selection; once it has
          // read it, the send panel underneath can go.
          ov.openSend(null, files, files.slice(0, 3));
          ov.setShare(true);
          await sleep(400);
          S.useOverlays.getState().closeSend();
          break;
      }
    },
    { quiet: !!shot.quiet, scene: shot.scene ?? null },
  );
}

/** Frame a raw screenshot like the app window and encode it, using the
 *  browser's own canvas — no image library needed. */
async function frame(page, png, dark) {
  const m = MARGIN * SCALE;
  const w = WINDOW.width * SCALE;
  const h = WINDOW.height * SCALE;
  const dataUrl = await page.evaluate(
    async ({ src, m, w, h, r, dark, q }) => {
      const img = new Image();
      img.src = src;
      await img.decode();
      const cv = document.createElement("canvas");
      cv.width = w + 2 * m;
      cv.height = h + 2 * m;
      const ctx = cv.getContext("2d");
      const path = new Path2D();
      path.roundRect(m, m, w, h, r);
      ctx.save();
      ctx.shadowColor = dark ? "rgba(0,0,0,0.55)" : "rgba(35,40,48,0.22)";
      ctx.shadowBlur = m * 0.9;
      ctx.shadowOffsetY = m * 0.3;
      ctx.fillStyle = "#000";
      ctx.fill(path);
      ctx.restore();
      ctx.save();
      ctx.clip(path);
      ctx.drawImage(img, m, m, w, h);
      ctx.restore();
      ctx.lineWidth = 2;
      ctx.strokeStyle = dark ? "rgba(255,255,255,0.10)" : "rgba(35,40,48,0.10)";
      ctx.stroke(path);
      return cv.toDataURL("image/webp", q);
    },
    {
      src: `data:image/png;base64,${png.toString("base64")}`,
      m,
      w,
      h,
      r: RADIUS * SCALE,
      dark,
      q: QUALITY,
    },
  );
  return Buffer.from(dataUrl.split(",")[1], "base64");
}

async function main() {
  const server = await createServer({
    root,
    logLevel: "warn",
    server: { port: 14_200, strictPort: false },
  });
  await server.listen();
  const base = server.resolvedUrls.local[0].replace(/\/$/, "");
  const browser = await launchBrowser();
  let n = 0;
  try {
    for (const lang of langs) {
      await mkdir(join(OUT, lang), { recursive: true });
      for (const shot of SHOTS) {
        if (only && !only.has(shot.name)) continue;
        for (const theme of shot.themes ?? ["light"]) {
          if (opts.theme && opts.theme !== theme) continue;
          const ctx = await browser.newContext({
            viewport: WINDOW,
            deviceScaleFactor: SCALE,
            colorScheme: theme,
            locale: lang === "zh" ? "zh-CN" : "en-US",
            timezoneId: TIME_ZONE,
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
          await page.goto(`${base}/?demo=1#${shot.route}`, {
            waitUntil: "networkidle",
          });
          await page.evaluate(() => document.fonts.ready);
          await page.waitForTimeout(500);
          await stage(page, shot);
          if (shot.scene === "share")
            await page.waitForFunction(() =>
              document.body.innerText.includes(":51705/s/"),
            );
          // entrance animations (modals rise, the radar settles)
          await page.waitForTimeout(900);
          const raw = await page.screenshot({ type: "png" });
          const file = join(
            OUT,
            lang,
            `${shot.name}${theme === "dark" ? "-dark" : ""}.webp`,
          );
          await writeFile(file, await frame(page, raw, theme === "dark"));
          console.log(`  ${file.slice(root.length + 1)}`);
          n++;
          await ctx.close();
        }
      }
    }
  } finally {
    await browser.close();
    await server.close();
  }
  console.log(`${n} screenshot(s) written.`);
}

if (!existsSync(join(root, "node_modules"))) {
  console.error("Run `pnpm install` first.");
  process.exit(1);
}
main().catch((e) => {
  console.error(e);
  process.exit(1);
});
