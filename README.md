# LanBeam

**English** · [简体中文](README.zh-CN.md)

> Fast, private, peer‑to‑peer file transfer over your local network — no cloud, no account, no relay.

**LanBeam** sends files (and quick text) directly between devices on the same LAN. Every
transfer runs over an end‑to‑end encrypted channel and never touches the internet. For a
device that doesn't have LanBeam installed, it can also serve a one‑time, link‑based
download over plain HTTP — so a phone browser can grab a file with no app at all.

Built with **Tauri 2** (a Rust core + a WebView UI). The shipped binary is a single
self‑contained executable (~8 MB); the web runtime is the system WebView2 on Windows, so
nothing extra is bundled.

- **Status:** `v0.1.0` — feature‑complete, pre‑release.
- **Platforms:** Windows (primary target); macOS / Linux are supported by the Tauri stack.
- **Languages:** English & 简体中文 (in‑app, switchable).

---

## Features

### Transfer
- **Direct P2P over the LAN** — devices find each other via a lightweight custom UDP
  discovery protocol (not mDNS) and connect directly; no server in the middle.
- **End‑to‑end encryption** — every session is a `Noise_XX_25519_ChaChaPoly_BLAKE2s`
  handshake (via the [`snow`](https://crates.io/crates/snow) crate). Devices are
  identified by a static X25519 key; a short **SAS** (safety string) lets you verify a
  peer out‑of‑band.
- **Integrity verified** — files are streamed through SHA‑256 and re‑hashed on the
  receiver before they're published (toggleable).
- **Resume, pause & cancel** — interrupted transfers resume from a persisted byte offset;
  pause applies backpressure (bounded, auto‑resumes); cancel frees the slot immediately.
- **Conflict policies** — `rename` (de‑dupe), `overwrite` (crash‑safe: streams to a temp
  and atomically replaces only on full success), or `ask`.
- **Auto‑organize** received files by sending device or by date.
- **Concurrency cap & rate limiting**, plus per‑file progress.

### Pairing & reach
- **Pairing** — a 6‑digit code (10‑minute TTL), a scannable QR, or a `lanbeam://pair`
  deep link. Pairing pins fingerprints on both sides for silent auto‑recognition later.
- **IP direct connect** for peers that automatic discovery can't see (different subnet).
- **Quick text** — send a snippet/link over the encrypted channel, optionally dropped on
  the receiver's clipboard.
- **Browser receive** — publish an explicit file set over a one‑shot HTTP share: an
  unguessable token URL with a TTL, a download cap, and instant stop — LAN‑only, files
  addressed by index (no path/traversal surface).

### Privacy & system integration
- **Metadata stripping** — remove EXIF / ICC / XMP from JPEG/PNG/WebP on send, via
  container surgery (no re‑encode; pixels stay byte‑identical).
- **Trust store** — remembered peers (`deviceId → name, auto‑accept, paired‑at`) with a
  fingerprint‑changed alert.
- System tray + close‑to‑tray, native notifications, launch‑at‑login, an opt‑in global
  hotkey, network‑interface filtering, and one‑click identity reset.

---

## Security model (at a glance)

- The device's X25519 private key lives in the **OS keychain** (`keyring`), never on disk
  in plaintext.
- All peer traffic is encrypted end‑to‑end (Noise); the app opens no internet connections.
- Untrusted input is treated as hostile: manifest names/sizes/counts are bounded, received
  paths are sanitized against traversal and Windows reserved‑name/ADS tricks, and pairing
  is TOFU + user‑confirmed with an out‑of‑band SAS check. A `lanbeam://` deep link only
  ever *pre‑fills* the pairing form — it never grants trust on its own.
- The browser‑share server binds the LAN, serves only the explicit file set by index, and
  is gated by the token + TTL + download‑cap on every request.

Dependency advisories are tracked with `cargo audit`; accepted transitive findings are
documented in [`src-tauri/.cargo/audit.toml`](src-tauri/.cargo/audit.toml).

---

## Tech stack

| Layer | Choices |
|---|---|
| Shell | Tauri 2 (Rust) + WebView2 |
| Backend | Rust, `tokio`, `snow` (Noise), custom UDP discovery, `axum` (share server), `img-parts` (EXIF), `keyring`, `sha2` |
| Frontend | React 19, TypeScript (strict), Vite, `zustand`, `react-i18next` |
| Tooling | pnpm, Biome (lint/format), Vitest, `cargo clippy` / `rustfmt` / `cargo-llvm-cov` |

---

## Getting started

### Prerequisites
- [Rust](https://www.rust-lang.org/tools/install) — stable toolchain (≥ 1.85)
- [Node.js](https://nodejs.org) ≥ 20.19 and [pnpm](https://pnpm.io) (`npm i -g pnpm`)
- The [Tauri 2 system prerequisites](https://tauri.app/start/prerequisites/) for your OS
  (on Windows: the MSVC build tools + the WebView2 runtime, which ships with Windows 10/11).

### Install
```bash
pnpm install
```

### Develop
```bash
pnpm tauri dev      # run the desktop app with hot‑reload
# or just the web UI in a browser (backend calls fall back to demo stubs):
pnpm dev
```

### Build
```bash
pnpm tauri build    # produces the app binary + an NSIS installer under
                    # src-tauri/target/release/ (and .../bundle/nsis/)
```

---

## Project layout

```
.
├── src/                  # React + TypeScript frontend
│   ├── bridge/api.ts     #   typed wrappers over Tauri commands/events (+ browser stubs)
│   ├── lib/store.ts      #   zustand stores (persisted where relevant)
│   ├── components/       #   modals, shell, shared UI primitives
│   ├── pages/            #   Devices / Transfers / Inbox / Trusted / Settings
│   └── i18n/             #   en + zh locales
├── src-tauri/            # Rust backend (the app core)
│   └── src/
│       ├── discovery/    #   UDP LAN discovery + interface enumeration
│       ├── transport/    #   Noise handshake + framing
│       ├── transfer.rs   #   send/receive state machine, resume, integrity, conflicts
│       ├── share.rs      #   axum one‑shot browser‑share server
│       ├── sanitize.rs   #   received‑path safety (the single write choke point)
│       ├── trust.rs      #   trust store  ·  exif.rs — metadata stripping
│       └── commands.rs   #   the Tauri command surface
├── ROADMAP.md            # backend milestones M4–M9 (all shipped)
└── vitest.config.ts
```

---

## Testing

```bash
# Frontend (Vitest + Testing Library)
pnpm test
pnpm test:coverage

# Backend (from src-tauri/)
cargo test
cargo clippy --all-targets
cargo fmt --check
cargo llvm-cov --summary-only -- --test-threads=1   # coverage
```

> **Windows note:** run backend tests serially (`-- --test-threads=1`). The MockRuntime
> integration tests can intermittently crash (`0xc0000005`) during native teardown when run
> in parallel — that's an environmental flake, not a logic failure; just re‑run.

---

## Packaging & distribution

- `pnpm tauri build` emits a single **NSIS** installer. The raw
  `src-tauri/target/release/lanbeam.exe` also runs standalone (a "green"/portable copy).
- **WebView2 is not bundled** — the app uses the system Evergreen runtime, which is present
  on Windows 10/11.
- The `lanbeam://` protocol is registered by the installer; a portable build self‑registers
  the scheme (per‑user, HKCU) the first time it runs.

---

## License

Released under the [MIT License](LICENSE). Third‑party components and their licenses are
listed in the app under **Settings → About → Open‑source licenses**.
