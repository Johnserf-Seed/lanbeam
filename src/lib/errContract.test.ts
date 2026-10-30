// CROSS-LANGUAGE CONTRACT TEST: backend error kinds ↔ the UI's words for them.
//
// A failed command rejects with `{kind, message}`; the UI shows the localized
// text for `kind` and never the message (sendops ERR_KEYS). A `transfer_error`
// carries `ui_code()`; the UI maps that (TRANSFER_ERR_KEYS). Neither side's own
// tests see a kind added in Rust without its UI line — it just falls through
// to "something went wrong", which is how a wrong pairing code and an
// unreachable peer ended up reading as "the connection failed" and "a file or
// disk error". So read error.rs itself and hold both tables to it.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import en from "../i18n/locales/en.json";
import zh from "../i18n/locales/zh.json";
import { ERR_KEYS, TRANSFER_ERR_KEYS } from "./sendops";

const rust = readFileSync(
  resolve(process.cwd(), "src-tauri/src/error.rs"),
  "utf8",
);

/** Every variant of `pub enum LanBeamError { … }`. */
function variants(): string[] {
  const start = rust.indexOf("pub enum LanBeamError {");
  expect(start, "LanBeamError not found — did error.rs move?").toBeGreaterThan(
    -1,
  );
  const body = rust.slice(start, rust.indexOf("\n}\n", start));
  return [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*)\s*[(,{]/gm)].map(
    (m) => m[1],
  );
}

/** Every code `ui_code()` can return (the arms' string literals). */
function uiCodes(): string[] {
  const start = rust.indexOf("pub fn ui_code(&self)");
  expect(start, "ui_code() not found — did error.rs move?").toBeGreaterThan(-1);
  const body = rust.slice(start, rust.indexOf("\n    }\n", start));
  return [...new Set([...body.matchAll(/=>\s*"([a-z_]+)"/g)].map((m) => m[1]))];
}

function flatKeys(o: Record<string, unknown>, p = ""): string[] {
  return Object.entries(o).flatMap(([k, v]) =>
    v && typeof v === "object"
      ? flatKeys(v as Record<string, unknown>, `${p}${k}.`)
      : [`${p}${k}`],
  );
}
const EN = new Set(flatKeys(en));
const ZH = new Set(flatKeys(zh));

describe("the backend error contract", () => {
  it("parses the enum it guards (so a parser bug can't pass the rest)", () => {
    const v = variants();
    expect(v).toEqual(expect.arrayContaining(["Io", "Protocol", "Timeout"]));
    expect(v).toEqual(
      expect.arrayContaining([
        "PairRejected",
        "InvalidAddress",
        "PairCodeMissing",
        "Unreachable",
      ]),
    );
    expect(uiCodes()).toEqual(
      expect.arrayContaining(["declined", "io", "protocol", "unreachable"]),
    );
  });

  it("every LanBeamError kind has its own localized message", () => {
    expect(variants().filter((k) => !ERR_KEYS[k])).toEqual([]);
  });

  it("…and the table keeps no line for a kind that no longer exists", () => {
    const v = new Set(variants());
    expect(Object.keys(ERR_KEYS).filter((k) => !v.has(k))).toEqual([]);
  });

  it("every code a transfer_error can carry has a message", () => {
    expect(uiCodes().filter((c) => !TRANSFER_ERR_KEYS[c])).toEqual([]);
  });

  it("every message those tables point at exists in BOTH languages", () => {
    const keys = [
      ...new Set([
        ...Object.values(ERR_KEYS),
        ...Object.values(TRANSFER_ERR_KEYS),
        "errors.generic",
      ]),
    ];
    expect(keys.filter((k) => !EN.has(k))).toEqual([]);
    expect(keys.filter((k) => !ZH.has(k))).toEqual([]);
  });

  it("the two languages carry exactly the same keys", () => {
    expect([...EN].filter((k) => !ZH.has(k))).toEqual([]);
    expect([...ZH].filter((k) => !EN.has(k))).toEqual([]);
  });
});
