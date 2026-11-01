// CROSS-LANGUAGE CONTRACT TEST: the JSON the backend serializes ↔ the types the
// UI reads it through.
//
// Each Rust struct below reaches the webview through serde; its TS twin in
// bridge/api.ts is hand-written. A field that leaves Rust under a different
// name than the UI expects reads as `undefined` with no error anywhere — which
// is exactly how `SettingsDto.ui_zoom` (no `rename = "uiZoom"`) made the first
// Ctrl+= ZOOM OUT and left the zoom select blank (H12). So parse both sides and
// require the same field names.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const read = (p: string) => readFileSync(resolve(process.cwd(), p), "utf8");

const snakeToCamel = (s: string) =>
  s.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

/** Drop `/* … */ ` and `; // …` comments — doc comments mention `{braces}`. */
const stripComments = (s: string) =>
  s.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");

/** The text between the `{` at `open` and its matching `}`. */
function braceBody(src: string, open: number): string {
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return src.slice(open + 1, i);
  }
  throw new Error("unbalanced braces");
}

/** The JSON field names `pub struct <name>` serializes to: the field name, or
 *  its `#[serde(rename = "…")]`, under a struct-level `rename_all = "camelCase"`
 *  when there is one. `#[serde(skip)]`/`skip_serializing` fields never appear. */
function rustJsonFields(file: string, name: string): string[] {
  const src = stripComments(read(file));
  const m = new RegExp(`pub struct ${name}\\s*\\{`).exec(src);
  expect(m, `${name} not found in ${file}`).toBeTruthy();
  const at = (m as RegExpExecArray).index;
  // The attributes right above the struct: back to the previous item's end.
  const before = src.slice(0, at);
  const attrsAbove = before.slice(
    Math.max(before.lastIndexOf("}"), before.lastIndexOf(";")) + 1,
  );
  const camel = /rename_all\s*=\s*"camelCase"/.test(attrsAbove);
  const body = braceBody(src, at + (m as RegExpExecArray)[0].length - 1);
  const out: string[] = [];
  let attrs = "";
  let depth = 0;
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (depth > 0 || t.startsWith("#[")) {
      attrs += ` ${t}`;
      depth +=
        (t.match(/[([]/g) ?? []).length - (t.match(/[)\]]/g) ?? []).length;
      continue;
    }
    const f = /^(?:pub(?:\([^)]*\))?\s+)?([a-z_][a-z0-9_]*)\s*:/.exec(t);
    if (f) {
      const skipped = /serde\([^)]*\bskip(?:_serializing)?\b(?!_)/.test(attrs);
      const rename = /rename\s*=\s*"([^"]+)"/.exec(attrs);
      if (!skipped)
        out.push(rename ? rename[1] : camel ? snakeToCamel(f[1]) : f[1]);
      attrs = "";
    }
  }
  return out.sort();
}

/** The top-level keys of `export type <name> = { … }` in bridge/api.ts —
 *  one-line or multi-line, nested object types collapsed away. */
function tsTypeKeys(name: string): string[] {
  const src = stripComments(read("src/bridge/api.ts"));
  const m = new RegExp(`export type ${name} = \\{`).exec(src);
  expect(m, `type ${name} not found in api.ts`).toBeTruthy();
  let body = braceBody(
    src,
    (m as RegExpExecArray).index + (m as RegExpExecArray)[0].length - 1,
  );
  while (/\{[^{}]*\}/.test(body)) body = body.replace(/\{[^{}]*\}/g, "");
  return [...body.matchAll(/(?:^|[;\n])\s*([A-Za-z_]\w*)\??\s*:/g)]
    .map((x) => x[1])
    .sort();
}

/** [Rust file, Rust struct, TS type] — every DTO the UI reads by name. */
const PAIRS: [string, string, string][] = [
  ["src-tauri/src/commands.rs", "MyIdentity", "MyIdentity"],
  ["src-tauri/src/commands.rs", "SettingsDto", "Settings"],
  ["src-tauri/src/commands.rs", "PairingInvite", "PairingInvite"],
  ["src-tauri/src/commands.rs", "ConnectResult", "ConnectResult"],
  ["src-tauri/src/commands.rs", "NetworkInfo", "NetworkInfo"],
  ["src-tauri/src/commands.rs", "ShareStarted", "ShareStarted"],
  ["src-tauri/src/commands.rs", "ShareUpdated", "ShareUpdated"],
  ["src-tauri/src/commands.rs", "ShareEntry", "ShareEntry"],
  ["src-tauri/src/commands.rs", "PartialDto", "PartialFile"],
  ["src-tauri/src/trust.rs", "TrustedPeerDto", "TrustedPeer"],
  ["src-tauri/src/share.rs", "ShareDownloadEvent", "ShareDownloadEvent"],
  ["src-tauri/src/discovery/mod.rs", "DiscoveredDevice", "DiscoveredDevice"],
  ["src-tauri/src/state.rs", "NetDegraded", "NetDegradedEvent"],
];

describe("the backend DTO contract", () => {
  it("reads both sides (so a parser bug can't pass the rest)", () => {
    expect(
      rustJsonFields("src-tauri/src/commands.rs", "PairingInvite"),
    ).toEqual(["addr", "code", "expiresAt", "qr"]);
    expect(tsTypeKeys("PairingInvite")).toEqual([
      "addr",
      "code",
      "expiresAt",
      "qr",
    ]);
  });

  it.each(PAIRS)(
    "%s %s serializes exactly the fields api.ts's %s declares",
    (file, rust, ts) => {
      expect(rustJsonFields(file, rust)).toEqual(tsTypeKeys(ts));
    },
  );
});
