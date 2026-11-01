// CROSS-LANGUAGE CONTRACT TEST: the names on both sides of the IPC boundary.
//
// A command the UI invokes under a name Rust doesn't register fails only when
// someone clicks it; an event Rust emits under a name nothing listens for, or a
// listener for a name nothing emits, fails silently forever. Neither side's own
// tests can see the other, so read both sources and hold the names together.
// (The tray's computed `tray:<id>` events have their own test,
// trayContract.test.ts.)
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = (p: string) => resolve(process.cwd(), p);

/** Every non-test source file under `dir` with one of `exts`. */
function sources(dir: string, exts: string[]): string[] {
  return readdirSync(root(dir), { recursive: true, encoding: "utf8" })
    .filter((f) => exts.some((e) => f.endsWith(e)) && !f.includes(".test."))
    .map((f) => readFileSync(join(root(dir), f), "utf8"));
}

/** Rust source without its unit tests (they emit and register for themselves). */
const rust = sources("src-tauri/src", [".rs"]).map((s) => {
  const i = s.indexOf("#[cfg(test)]\nmod tests");
  return i >= 0 ? s.slice(0, i) : s;
});
const ts = sources("src", [".ts", ".tsx"]);

const all = (texts: string[], re: RegExp) =>
  new Set(texts.flatMap((s) => [...s.matchAll(re)].map((m) => m[1])));

describe("the IPC contract", () => {
  it("every command the UI invokes is registered, and every registered one is used", () => {
    const lib = readFileSync(root("src-tauri/src/lib.rs"), "utf8");
    const list = lib.match(/generate_handler!\[([\s\S]*?)\]/)?.[1];
    expect(list, "generate_handler! not found — did lib.rs move?").toBeTruthy();
    const registered = new Set(
      (list ?? "")
        .split(",")
        .map((x) => x.trim().split("::").pop() ?? "")
        .filter((x) => /^[a-z_]+$/.test(x)),
    );
    const invoked = all(ts, /\binvoke(?:<[^>]*>)?\(\s*"([a-z_]+)"/g);
    expect(registered.size).toBeGreaterThan(40);
    expect([...invoked].filter((c) => !registered.has(c))).toEqual([]);
    expect([...registered].filter((c) => !invoked.has(c))).toEqual([]);
  });

  it("every event the backend emits is listened for, and every listener has an emitter", () => {
    const emitted = all(rust, /\.emit\(\s*"([a-z_:-]+)"/g);
    const listened = new Set(
      [...all(ts, /\bonEvent(?:<[^>]*>)?\(\s*"([a-z_:-]+)"/g)].filter(
        (e) => !e.startsWith("tray:"),
      ),
    );
    expect(emitted.size).toBeGreaterThan(15);
    expect([...listened].filter((e) => !emitted.has(e))).toEqual([]);
    expect([...emitted].filter((e) => !listened.has(e))).toEqual([]);
  });
});
