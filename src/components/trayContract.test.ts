// CROSS-LANGUAGE CONTRACT TEST.
//
// The tray is split across the language boundary: Rust decides which menu ids
// get handed to the webview (`route()` → Action::Emit / EmitWithWindow, emitted
// as `tray:<id>`), and AppShell listens for those exact event names. Neither
// side's own tests can catch a mismatch — a renamed id or a typo'd listener just
// makes that menu entry silently do nothing, which is exactly the class of bug
// that ships unnoticed.
//
// So read BOTH sources and assert the two sets are identical.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const read = (p: string) => readFileSync(resolve(process.cwd(), p), "utf8");

/** Ids Rust hands to the webview (it emits `tray:<id>` for each). */
function idsRustEmits(): Set<string> {
  const rust = read("src-tauri/src/tray.rs");
  const start = rust.indexOf("fn route(id: &str) -> Action {");
  expect(start, "route() not found — did tray.rs move?").toBeGreaterThan(-1);
  const body = rust.slice(start, rust.indexOf("\n}", start));
  const ids = new Set<string>();
  // Match arms like:  "a" | "b" => Action::Emit,   /   … => { Action::EmitWithWindow }
  for (const arm of body.matchAll(
    /((?:"[a-z_]+"\s*\|?\s*)+)=>\s*\{?\s*Action::(?:EmitWithWindow|Emit)\b/g,
  )) {
    for (const lit of arm[1].matchAll(/"([a-z_]+)"/g)) ids.add(lit[1]);
  }
  return ids;
}

/** Ids the menu is actually BUILT with — parsed from `build()` itself, so this is
 *  derived from the real menu rather than a hand-copied list that can rot. */
function idsBuilt(): Set<string> {
  const rust = read("src-tauri/src/tray.rs");
  const start = rust.indexOf("pub fn build(app: &tauri::App)");
  expect(start, "build() not found — did tray.rs move?").toBeGreaterThan(-1);
  const body = rust.slice(start, rust.indexOf("\n}", start));
  return new Set(
    [
      ...body.matchAll(/(?:Check)?MenuItem::with_id\(\s*app,\s*"([a-z_]+)"/g),
    ].map((m) => m[1]),
  );
}

/** Ids AppShell actually listens for. */
function idsFrontendHandles(): Set<string> {
  const shell = read("src/components/AppShell.tsx");
  return new Set(
    [...shell.matchAll(/api\.onEvent\(\s*"tray:([a-z_]+)"/g)].map((m) => m[1]),
  );
}

describe("tray event contract (Rust ↔ frontend)", () => {
  it("every id Rust emits has a frontend handler, and vice versa", () => {
    const emitted = idsRustEmits();
    const handled = idsFrontendHandles();

    // Sanity: the parse actually found something (a silently-empty set would
    // make this test vacuously pass).
    expect(emitted.size).toBeGreaterThan(5);

    expect([...handled].sort()).toEqual([...emitted].sort());
  });

  it("every BUILT menu item is handled — natively or by the webview", () => {
    // Closes the last hole: an item added to build() with no route() arm and no
    // listener would otherwise pass every other test and ship as a menu entry
    // that does nothing when clicked.
    const built = idsBuilt();
    const emitted = idsRustEmits();
    const nativelyHandled = new Set(["show", "quit"]);

    expect(built.size).toBeGreaterThan(5); // guard against a vacuous parse
    for (const id of built) {
      // `status` is a deliberately disabled label, not a button.
      if (id === "status") continue;
      expect(
        nativelyHandled.has(id) || emitted.has(id),
        `menu item "${id}" is built but nothing handles it`,
      ).toBe(true);
    }
  });

  it("show/quit stay backend-only — they must NOT be delegated to the webview", () => {
    // A hidden window has no one to service them; Rust handles both natively.
    const emitted = idsRustEmits();
    const handled = idsFrontendHandles();
    for (const id of ["show", "quit"]) {
      expect(emitted.has(id), `${id} must not be emitted`).toBe(false);
      expect(handled.has(id), `${id} must not be handled in the webview`).toBe(
        false,
      );
    }
  });
});
