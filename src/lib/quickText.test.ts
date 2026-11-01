// quickTextSize mirrors the backend's two quick-text checks (the byte cap in
// commands/transfer.rs, the frame cap in transfer/wire.rs). The constants are
// hand-copied, so the contract half of this file reads consts.rs and fails the
// moment either side moves.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { MAX_TEXT_BYTES, quickTextSize } from "./quickText";

describe("quickTextSize", () => {
  it("counts UTF-8 bytes, not characters", () => {
    expect(quickTextSize("hello")).toEqual({ bytes: 5, fits: true });
    // each CJK character is 3 bytes on the wire
    expect(quickTextSize("快传")).toEqual({ bytes: 6, fits: true });
  });

  it("refuses anything past the byte cap", () => {
    expect(quickTextSize("a".repeat(MAX_TEXT_BYTES)).fits).toBe(true);
    expect(quickTextSize("a".repeat(MAX_TEXT_BYTES + 1)).fits).toBe(false);
    // 20,481 CJK chars: well under 60K characters, over 60 KiB of bytes — the
    // case that used to surface as "connection failed"
    const cjk = "字".repeat(20_481);
    expect(quickTextSize(cjk)).toEqual({ bytes: 61_443, fits: false });
  });

  it("refuses a text under the byte cap whose escaped frame overflows", () => {
    // 40 KiB of quotes: under MAX_TEXT_BYTES, but each escapes to 2 bytes
    const quotes = '"'.repeat(40 * 1024);
    const r = quickTextSize(quotes);
    expect(r.bytes).toBeLessThanOrEqual(MAX_TEXT_BYTES);
    expect(r.fits).toBe(false);
  });
});

describe("quick-text limits match consts.rs", () => {
  const rust = readFileSync(
    resolve(process.cwd(), "src-tauri/src/consts.rs"),
    "utf8",
  );

  it("MAX_TEXT_BYTES", () => {
    const m = rust.match(/pub const MAX_TEXT_BYTES: usize = (\d+) \* 1024;/);
    expect(m, "MAX_TEXT_BYTES not found — did consts.rs change?").toBeTruthy();
    expect(MAX_TEXT_BYTES).toBe(Number(m?.[1]) * 1024);
  });

  it("MAX_PLAINTEXT", () => {
    const m = rust.match(/pub const MAX_PLAINTEXT: usize = (\d+);/);
    expect(m, "MAX_PLAINTEXT not found — did consts.rs change?").toBeTruthy();
    // the frame cap itself isn't exported; probe it at the exact boundary
    // instead: a frame of exactly MAX_PLAINTEXT fits, one byte more does not.
    const cap = Number(m?.[1]);
    const overhead = 1 + 24 + 22 + 2; // kind + wrapper + the string's quotes
    const n = cap - overhead; // plain ASCII: escaped length = n + 2
    // n > MAX_TEXT_BYTES, so build the boundary out of 2-byte escapes instead
    const escapes = Math.floor(n / 2);
    const pad = n - escapes * 2;
    const atCap = '"'.repeat(escapes) + "a".repeat(pad);
    expect(quickTextSize(atCap).fits).toBe(true);
    expect(quickTextSize(`${atCap}a`).fits).toBe(false);
  });
});
