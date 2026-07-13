/** Third-party open-source components LanBeam is built on, shown in the
 *  Settings → 关于 → 开源许可 modal. Each entry is a DIRECT dependency: the
 *  frontend (package.json) plus the Rust backend (src-tauri/Cargo.toml).
 *
 *  Versions are the resolved ones (node_modules / Cargo.lock); the license is
 *  the SPDX identifier each project declares in its own manifest. Transitive
 *  dependencies aren't enumerated here — their full license texts ship with the
 *  source tree. LanBeam itself is MIT-licensed. Keep this list in step with the
 *  two manifests when a direct dependency is added, dropped, or bumped. */
export type LicenseEntry = {
  name: string;
  version: string;
  license: string;
};

export const LICENSES: LicenseEntry[] = [
  // ── frontend · package.json dependencies ──
  { name: "react", version: "19.2.7", license: "MIT" },
  { name: "react-dom", version: "19.2.7", license: "MIT" },
  { name: "react-router-dom", version: "7.18.1", license: "MIT" },
  { name: "zustand", version: "5.0.14", license: "MIT" },
  { name: "i18next", version: "26.3.6", license: "MIT" },
  { name: "react-i18next", version: "17.0.9", license: "MIT" },
  {
    name: "i18next-browser-languagedetector",
    version: "8.2.1",
    license: "MIT",
  },
  { name: "qrcode", version: "1.5.4", license: "MIT" },
  { name: "@tauri-apps/api", version: "2.11.1", license: "Apache-2.0 OR MIT" },
  {
    name: "@tauri-apps/plugin-dialog",
    version: "2.7.1",
    license: "MIT OR Apache-2.0",
  },
  {
    name: "@tauri-apps/plugin-opener",
    version: "2.5.4",
    license: "MIT OR Apache-2.0",
  },
  {
    name: "@tauri-apps/plugin-store",
    version: "2.4.3",
    license: "MIT OR Apache-2.0",
  },

  // ── backend · src-tauri/Cargo.toml dependencies ──
  { name: "tauri", version: "2.11.5", license: "Apache-2.0 OR MIT" },
  { name: "snow", version: "0.9.6", license: "Apache-2.0 OR MIT" },
  { name: "tokio", version: "1.52.3", license: "MIT" },
  { name: "tokio-util", version: "0.7.18", license: "MIT" },
  { name: "serde", version: "1.0.228", license: "MIT OR Apache-2.0" },
  { name: "serde_json", version: "1.0.150", license: "MIT OR Apache-2.0" },
  { name: "axum", version: "0.7.9", license: "MIT" },
  { name: "img-parts", version: "0.3.3", license: "MIT OR Apache-2.0" },
  { name: "sha2", version: "0.10.9", license: "MIT OR Apache-2.0" },
  { name: "keyring", version: "3.6.3", license: "MIT OR Apache-2.0" },
  { name: "base64", version: "0.22.1", license: "MIT OR Apache-2.0" },
  { name: "zeroize", version: "1.9.0", license: "Apache-2.0 OR MIT" },
  { name: "socket2", version: "0.5.10", license: "MIT OR Apache-2.0" },
  { name: "if-addrs", version: "0.13.4", license: "MIT OR BSD-3-Clause" },
  { name: "getrandom", version: "0.4.3", license: "MIT OR Apache-2.0" },
  {
    name: "dunce",
    version: "1.0.5",
    license: "CC0-1.0 OR MIT-0 OR Apache-2.0",
  },
  { name: "thiserror", version: "1.0.69", license: "MIT OR Apache-2.0" },
  { name: "uuid", version: "1.23.4", license: "Apache-2.0 OR MIT" },
  { name: "log", version: "0.4.33", license: "MIT OR Apache-2.0" },
  {
    name: "tauri-plugin-opener",
    version: "2.5.4",
    license: "Apache-2.0 OR MIT",
  },
  {
    name: "tauri-plugin-dialog",
    version: "2.7.1",
    license: "Apache-2.0 OR MIT",
  },
  {
    name: "tauri-plugin-store",
    version: "2.4.3",
    license: "Apache-2.0 OR MIT",
  },
  {
    name: "tauri-plugin-clipboard-manager",
    version: "2.3.2",
    license: "Apache-2.0 OR MIT",
  },
  { name: "tauri-plugin-log", version: "2.8.0", license: "Apache-2.0 OR MIT" },
  {
    name: "tauri-plugin-single-instance",
    version: "2.4.2",
    license: "Apache-2.0 OR MIT",
  },
  {
    name: "tauri-plugin-notification",
    version: "2.3.3",
    license: "Apache-2.0 OR MIT",
  },
  {
    name: "tauri-plugin-autostart",
    version: "2.5.1",
    license: "Apache-2.0 OR MIT",
  },
  {
    name: "tauri-plugin-global-shortcut",
    version: "2.3.2",
    license: "Apache-2.0 OR MIT",
  },
];
