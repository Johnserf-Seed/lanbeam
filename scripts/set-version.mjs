#!/usr/bin/env node
// Set the app version everywhere it is recorded, in one go:
//
//   pnpm version:set 0.2.0
//
// package.json · src-tauri/tauri.conf.json · src-tauri/Cargo.toml · Cargo.lock.
// The release workflow refuses to build a tag that disagrees with any of the
// first three, so bump with this rather than by hand.
//
// Only `major.minor.patch` (optionally `-N`, N ≤ 65535) is accepted: the Windows
// MSI format has no room for `-beta.1`, and Tauri's bundler rejects it.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const version = (process.argv[2] ?? "").replace(/^v/, "");

const m = /^\d+\.\d+\.\d+(?:-(\d+))?$/.exec(version);
if (!m || (m[1] !== undefined && Number(m[1]) > 65535)) {
  console.error(
    "usage: pnpm version:set <major.minor.patch>   (e.g. 0.2.0, or 0.2.0-1 for a pre-release)",
  );
  process.exit(1);
}

/** Replace the first match of `re` (which must capture the text before and
 *  after the version) and fail loudly if the field has moved. */
function setIn(file, re) {
  const path = join(root, file);
  const text = readFileSync(path, "utf8");
  if (!re.test(text)) {
    console.error(
      `${file}: version field not found — update the pattern in scripts/set-version.mjs`,
    );
    process.exit(1);
  }
  writeFileSync(path, text.replace(re, `$1${version}$2`));
  console.log(`  ${file}`);
}

console.log(`Setting version ${version}:`);
// Top-level key only: two-space indent, first occurrence.
setIn("package.json", /^( {2}"version": ")[^"]*(")/m);
setIn("src-tauri/tauri.conf.json", /^( {2}"version": ")[^"]*(")/m);
// The [package] version is the only line in the manifest that starts with it.
setIn("src-tauri/Cargo.toml", /^(version = ")[^"]*(")/m);
setIn(
  "src-tauri/Cargo.lock",
  /(\[\[package\]\]\r?\nname = "lanbeam"\r?\nversion = ")[^"]*(")/,
);
console.log(
  `Done. Commit, then tag:  git tag v${version} && git push origin v${version}`,
);
