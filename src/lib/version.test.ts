// The version the app shows is the version it ships as (M14). The About section
// hard-coded "v0.1.0" — and still said so at 0.1.3 — because nothing tied it to
// the three files a release actually reads. Hold them all to APP_VERSION.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import en from "../i18n/locales/en.json";
import zh from "../i18n/locales/zh.json";
import { APP_VERSION } from "./version";

const read = (p: string) => readFileSync(resolve(process.cwd(), p), "utf8");

describe("the app version", () => {
  it("is one version everywhere a release records it", () => {
    expect(APP_VERSION).toMatch(/^\d+\.\d+\.\d+/);
    expect(JSON.parse(read("src-tauri/tauri.conf.json")).version).toBe(
      APP_VERSION,
    );
    expect(read("src-tauri/Cargo.toml")).toMatch(
      new RegExp(`^version = "${APP_VERSION.replace(/\./g, "\\.")}"$`, "m"),
    );
  });

  it("is never written into the UI text by hand", () => {
    for (const s of [
      ...Object.values(zh.settings),
      ...Object.values(en.settings),
    ]) {
      expect(s).not.toMatch(/\d+\.\d+\.\d+/);
    }
    // The one string that names a version takes it as a value (F2).
    expect(zh.settings.updateNewer).toContain("{{latest}}");
    expect(en.settings.updateNewer).toContain("{{latest}}");
    expect(read("src/pages/SettingsPage.tsx")).not.toMatch(/v\d+\.\d+\.\d+/);
  });
});
