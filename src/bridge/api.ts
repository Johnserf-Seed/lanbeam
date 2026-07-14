// Typed wrappers over the Tauri commands (Rust ↔ UI boundary).
// Every call is guarded so the UI also runs in a plain browser (vite dev /
// design review) with static demo data instead of a live backend.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type MyIdentity = {
  deviceId: string; // base64url X25519 public key (43 chars) — the pinning key
  shortId: string; // first 8 chars, display only
  name: string;
};

export type Settings = {
  deviceName: string;
  discoverable: boolean;
  autoOpenFolder: boolean;
  /** "errors" | "normal" | "verbose" — the file logger applies it on next launch */
  logLevel: string;
  /** "ask" | "trusted" | "all" — the backend consults this before prompting */
  recvPolicy: string;
  /** absolute path overriding the OS download folder; omitted = OS default (M5.2) */
  downloadDirOverride?: string;
  /** TCP listen port; 0 = LanBeam's default (51704) · applies on next launch */
  port: number;
  /** closing the window hides to the tray instead of quitting (M5.3) */
  trayClose: boolean;
  /** OS notifications for incoming prompts / finished receives (M5.4) */
  notifSystem: boolean;
  /** launch at login (M5.5); the backend mirrors it into the OS entry */
  autostart: boolean;
  /** discovery interface filter (IPv4 as shown by getNetworkInfo); omitted = all (M5.6) */
  ifaceFilter?: string;
  /** the global quick-summon hotkey is registered (M5.5); default off */
  hotkeyEnabled: boolean;
  /** the accelerator the quick-summon hotkey binds (M5.5 rebind), in "MOD+KEY"
   *  form (e.g. "Alt+Space", "Ctrl+Shift+K"); the UI formats it for display */
  hotkey: string;
  /** compute + verify a SHA-256 per file (M6.3); default on */
  verifyHash: boolean;
  /** name-collision policy (M6.5): "rename" | "overwrite" | "ask"; default "ask" */
  conflict: string;
  /** auto-organize mode (M6.6): "none" | "device" | "date"; default "none" */
  organize: string;
  /** concurrency cap (M6.7): how many transfers stream at once, 1–8; default 3 */
  maxConcurrent: number;
  /** per-transfer throughput cap (M6.7): "unlimited" or an MB/s count; default "unlimited" */
  rateLimit: string;
  /** interface scale; 1.0 = the design size. The webview is zoomed to this AND the
   *  window's minimum size scales with it — a zoom shrinks the CSS viewport, so a
   *  window floor that ignored it would let the layout be scaled off its own edge. */
  uiZoom: number;
  /** an incoming quick text is also written to this machine's clipboard when
   *  the sender asks for it (M7.3); default off (opt-in consent) */
  clipShare: boolean;
  /** strip photo metadata (location, camera, time) from JPEG/PNG/WebP images
   *  before they leave this device (M9.1); the persisted default the send
   *  confirm dialog can still override per transfer. Default on. */
  stripExif: boolean;
};

/** A fresh pairing invitation (start_pairing, M7.1): the 6-digit code plus the
 *  QR/deep-link payload another device can scan to join. */
export type PairingInvite = {
  code: string;
  qr: string;
};

/** The peer a pairing / IP-direct handshake reached (join_by_code /
 *  connect_by_addr, M7.1/7.2): its Device ID, friendly name, and the 6-digit
 *  SAS to compare out of band. */
export type ConnectResult = {
  deviceId: string;
  name: string;
  sas: string;
};

/** `pair_joined` (M7.1): a device redeemed this host's code. Carries the SAS so
 *  the host can offer the out-of-band compare (the MITM backstop for TOFU). */
export type PairJoinedEvent = {
  deviceId: string;
  name: string;
  sas: string;
};

/** `text_received` (M7.3): a quick text arrived. `at` is a millisecond unix
 *  stamp so the inbox can render a real receive time. */
export type TextReceivedEvent = {
  deviceId: string;
  senderName: string;
  text: string;
  at: number;
};

/** A browser fetched one file from a live share (`share_download`). Fired once
 *  per successful download so the app can surface it — toast, history entry,
 *  OS notification and a live count on the open share panel. `downloads` is the
 *  whole-set count so far (matches ShareEntry.downloads); `peerIp` is who pulled
 *  it. */
export type ShareDownloadEvent = {
  token: string;
  index: number;
  name: string;
  size: number;
  downloads: number;
  maxDownloads: number | null;
  fileCount: number;
  peerIp: string;
};

/** One local IPv4 endpoint (get_network_info, M5.1). */
export type NetworkInfo = {
  ip: string;
  /** null for interfaces without a broadcast address (e.g. P2P links) */
  broadcast: string | null;
};

/** One backend trust-store entry (list_trusted / trust_updated payload). */
export type TrustedPeer = {
  deviceId: string;
  name: string;
  autoAccept: boolean;
  /** unix seconds when the user first trusted this peer */
  pairedAt: number;
  /** unix seconds of the last accepted transfer from this peer */
  lastSeen: number;
};

export type DiscoveredDevice = {
  deviceId: string;
  name: string;
  address: string;
  port: number;
  /** In the list because someone typed its address (IP-direct / pair-by-code),
   *  not because it announced itself. A manual peer is the one kind that 删除设备
   *  can actually delete for good — a device broadcasting on the LAN comes back
   *  on its next announce, and offering to delete it promises what nothing can
   *  deliver. Additive: absent in browser-mode demo data. */
  manual?: boolean;
};

export type FileEntry = { name: string; size: number };
export type IncomingRequest = {
  sessionId: string;
  deviceId: string;
  sas: string;
  totalSize: number;
  fileCount: number;
  files: FileEntry[];
  /** the sender's self-declared friendly name (M4.2); absent from old peers */
  senderName?: string;
  /** true = the backend's receive policy already accepted — no reply expected */
  autoAccepted?: boolean;
  /** names that collide with an existing file in the download folder (M6.5);
   *  non-empty under the "ask" policy drives the ConflictModal */
  conflicts?: string[];
  /** the active name-collision policy for this session (M6.5): "rename" | "overwrite" | "ask" */
  conflictPolicy?: string;
  /** files that will continue from a persisted partial (M6.4) — informational */
  resuming?: { name: string; offset: number }[];
};

/** Per-file progress (M6.8): `transfer_file_progress`. `fileIndex` keys the
 *  detail drawer's per-file row (the receiver's on-disk name may differ). */
export type TransferFileProgressEvent = {
  sessionId: string;
  fileIndex: number;
  fileName: string;
  done: number;
  total: number;
  percent: number;
};

/** Per-file completion (M6.8): `transfer_file_done`. `verified` = the file
 *  carried a SHA-256 that matched (receive) / was attached (send). */
export type TransferFileDoneEvent = {
  sessionId: string;
  fileIndex: number;
  verified: boolean;
};

/** Machine-readable cause on transfer_error (M4.5) — the `error` string is
 *  human-oriented and NOT a stable contract, so never string-match it. */
export type TransferErrorCode =
  | "declined"
  | "timeout"
  | "cancelled"
  | "peer_too_old"
  | "integrity"
  | "io"
  | "protocol";

/** Wire-level transfer event payloads (UI enriches these in the store). */
export type TransferEvent = {
  sessionId: string;
  direction?: "send" | "receive";
  totalSize?: number;
  /** transfer_progress emits the size as `total` (with `done` bytes) */
  total?: number;
  done?: number;
  fileCount?: number;
  percent?: number;
  error?: string;
  code?: TransferErrorCode;
  savedNames?: string[];
};
/** connect_device (fingerprint re-verify) emits sas_code without a session. */
export type SasEvent = { sessionId?: string; sas: string; deviceId?: string };
/** Silent network degradations surfaced by the backend (M4.6).
 *  Known kinds: "udp_recv_fallback" | "tcp_port_fallback"; future kinds must
 *  not break the handler, so the type stays an open string. */
export type NetDegradedEvent = { kind: string; detail?: string };

/** A started browser share (start_share, M8.2): the access token, the LAN URL a
 *  browser opens, and the link's expiry (unix seconds). */
export type ShareStarted = {
  token: string;
  url: string;
  expiresAt: number;
};

/** The new expiry (unix seconds) after update_share changes a share's lifetime. */
export type ShareUpdated = {
  expiresAt: number;
};

/** One live browser share (list_shares, M8.2). `maxDownloads` is null for an
 *  unlimited link; `url` is empty only when the share server has no bound port. */
export type ShareEntry = {
  token: string;
  url: string;
  fileCount: number;
  totalSize: number;
  expiresAt: number;
  downloads: number;
  maxDownloads: number | null;
};

// ── demo fixtures for browser mode ──────────────────────────────────────
const DEMO_IDENTITY: MyIdentity = {
  deviceId: "vJx0Qm8dR3kePzW1bT5uYhN2aFcL7sG9oXiKM4EwD6A",
  shortId: "vJx0Qm8d",
  name: "书房 · MacBook Pro",
};
const DEMO_DEVICES: DiscoveredDevice[] = [
  {
    deviceId: "demo-mini",
    name: "客厅 · Mac mini",
    address: "192.168.1.23",
    port: 52637,
  },
  {
    deviceId: "demo-min",
    name: "小敏的手机",
    address: "192.168.1.41",
    port: 52637,
  },
  {
    deviceId: "demo-nas",
    name: "NAS · Synology",
    address: "192.168.1.9",
    port: 52637,
  },
  {
    deviceId: "demo-tp",
    name: "工位 · ThinkPad",
    address: "192.168.1.36",
    port: 52637,
  },
  {
    deviceId: "demo-ipad",
    name: "iPad Air",
    address: "192.168.1.57",
    port: 52637,
  },
];
let demoSettings: Settings = {
  deviceName: DEMO_IDENTITY.name,
  discoverable: true,
  autoOpenFolder: false,
  logLevel: "normal",
  recvPolicy: "trusted",
  port: 0,
  trayClose: true,
  notifSystem: true,
  autostart: false,
  hotkeyEnabled: false,
  hotkey: "Alt+Space",
  verifyHash: true,
  conflict: "ask",
  organize: "device",
  maxConcurrent: 3,
  rateLimit: "unlimited",
  uiZoom: 1,
  clipShare: false,
  stripExif: true,
};
const DEMO_NETWORK: NetworkInfo[] = [
  { ip: "192.168.1.20", broadcast: "192.168.1.255" },
];

// ── commands ────────────────────────────────────────────────────────────
export const getMyIdentity = () =>
  isTauri
    ? invoke<MyIdentity>("get_my_identity")
    : Promise.resolve({ ...DEMO_IDENTITY, name: demoSettings.deviceName });

export const getSettings = () =>
  isTauri
    ? invoke<Settings>("get_settings")
    : Promise.resolve({ ...demoSettings });

/** Every user-facing string in the tray menu plus its live state. The backend
 *  has no i18n layer, so the UI pushes the whole localized snapshot; the call is
 *  idempotent and re-sent whenever the language, device name, LAN IP or
 *  discoverability changes, so the menu can never drift out of sync. */
export type TraySync = {
  /** the disabled header line, e.g. "书房 · 192.168.1.20" */
  status: string;
  tooltip: string;
  show: string;
  send: string;
  quickText: string;
  share: string;
  pair: string;
  discoverable: string;
  openDir: string;
  inbox: string;
  transfers: string;
  settings: string;
  quit: string;
  /** whether the「可被发现」item shows its tick */
  isDiscoverable: boolean;
};

/** Push the localized labels + live state into the tray menu. A no-op in the
 *  browser demo (there is no tray). */
export const syncTray = (sync: TraySync) =>
  isTauri ? invoke<void>("sync_tray", { sync }) : Promise.resolve();

export const setDeviceName = (name: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, deviceName: name };
    return Promise.resolve();
  }
  return invoke<void>("set_device_name", { name });
};

export const setDiscoverable = (discoverable: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, discoverable };
    return Promise.resolve();
  }
  return invoke<void>("set_discoverable", { discoverable });
};

export const setAutoOpen = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, autoOpenFolder: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_auto_open", { enabled });
};

// M4.6: diagnostics log level ("errors" | "normal" | "verbose"); unknown
// values are silently ignored by the backend, mirroring set_recv_policy.
export const setLogLevel = (level: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, logLevel: level };
    return Promise.resolve();
  }
  return invoke<void>("set_log_level", { level });
};

// M4.4: inbound transfer policy ("ask" | "trusted" | "all") — the accept
// decision now lives in the backend, the UI only mirrors the value.
export const setRecvPolicy = (policy: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, recvPolicy: policy };
    return Promise.resolve();
  }
  return invoke<void>("set_recv_policy", { policy });
};

// M5.1: the machine's non-loopback IPv4 addresses (sorted by the backend).
export const getNetworkInfo = () =>
  isTauri
    ? invoke<NetworkInfo[]>("get_network_info")
    : Promise.resolve(DEMO_NETWORK);

// M5.2: point receiving at an existing directory; resolves to the canonical
// path as stored (and as get_download_dir reports from now on).
export const setDownloadDir = (path: string) =>
  isTauri
    ? invoke<string>("set_download_dir", { path })
    : Promise.resolve(path);

// M5.2: TCP listen port (0 = default) — persists only, applies on next launch.
// Out-of-range values are silently ignored backend-side, so validate first.
export const setListenPort = (port: number) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, port };
    return Promise.resolve();
  }
  return invoke<void>("set_listen_port", { port });
};

// M5.2: the port the listener is actually bound to right now — differs from
// the `port` setting until the next launch (and reflects an ephemeral
// fallback if the configured port was taken). Browser mode has no listener,
// so it echoes the effective default.
export const getListenPort = () =>
  isTauri
    ? invoke<number>("get_listen_port")
    : Promise.resolve(demoSettings.port || 51704);

// M5.3: close-to-tray toggle — read live on every CloseRequested.
export const setTrayClose = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, trayClose: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_tray_close", { enabled });
};

// M5.4: OS notification toggle — read live at fire time.
export const setNotifSystem = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, notifSystem: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_notif_system", { enabled });
};

// M5.5: launch at login. Can reject (the OS refused the entry) — callers
// must roll their optimistic toggle back on failure.
export const setAutostart = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, autostart: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_autostart", { enabled });
};

// M5.6: discovery interface filter — an IPv4 from getNetworkInfo, "" = all.
export const setIfaceFilter = (ip: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, ifaceFilter: ip || undefined };
    return Promise.resolve();
  }
  return invoke<void>("set_iface_filter", { ip });
};

// M5.5: register/unregister the global quick-summon hotkey immediately (no
// restart). Never rejects — a chord conflict is logged and the preference
// is still saved, so the toggle mirror stays authoritative.
export const setHotkeyEnabled = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, hotkeyEnabled: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_hotkey_enabled", { enabled });
};

// M5.5: rebind the global quick-summon accelerator (e.g. "Ctrl+Shift+K"). Bound
// live and effective immediately (no restart). Rejects when the accelerator is
// malformed, or — while the hotkey is enabled — when another app already owns the
// chord; on rejection the previous binding is kept, so callers surface the error
// and leave the displayed combo unchanged.
export const setHotkey = (combo: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, hotkey: combo };
    return Promise.resolve();
  }
  return invoke<void>("set_hotkey", { combo });
};

// M6.3: per-file SHA-256 integrity verification — read at send time, so it
// applies on the next transfer with no restart.
export const setVerifyHash = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, verifyHash: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_verify_hash", { enabled });
};

// M6.5: name-collision policy ("rename" | "overwrite" | "ask") — read at
// receive time. Unknown values are ignored backend-side (mirrors setRecvPolicy).
export const setConflictPolicy = (policy: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, conflict: policy };
    return Promise.resolve();
  }
  return invoke<void>("set_conflict_policy", { policy });
};

// M6.6: auto-organize mode ("none" | "device" | "date") — read at receive
// time. Unknown values are ignored backend-side.
export const setOrganize = (mode: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, organize: mode };
    return Promise.resolve();
  }
  return invoke<void>("set_organize", { mode });
};

// M6.7: concurrency cap (how many transfers stream at once) — clamped to 1–8
// backend-side, read live at each transfer's gate.
/** Interface scale. Applies immediately (webview zoom + a matching window floor)
 *  and persists. Out-of-range values are clamped by the backend, not refused. */
export const setUiZoom = (zoom: number) =>
  isTauri ? invoke<void>("set_ui_zoom", { zoom }) : Promise.resolve();

export const setMaxConcurrent = (max: number) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, maxConcurrent: max };
    return Promise.resolve();
  }
  return invoke<void>("set_max_concurrent", { max });
};

// M6.7: per-transfer throughput cap ("unlimited" or an MB/s count) — read when
// a transfer starts streaming. Invalid values ("0", junk) are ignored backend-side.
export const setRateLimit = (limit: string) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, rateLimit: limit };
    return Promise.resolve();
  }
  return invoke<void>("set_rate_limit", { limit });
};

// M7.3: whether an incoming quick text is also written to this machine's
// clipboard (when the sender asks for it) — read at receive time, so it
// applies to the very next text with no restart.
export const setClipShare = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, clipShare: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_clip_share", { enabled });
};

// M9.1: the persisted default for stripping photo metadata (location, camera,
// time) from images before sending — read at send time, so it applies to the
// next transfer with no restart. The confirm dialog can still override it per
// send. A plain boolean, so nothing to validate backend-side.
export const setStripExif = (enabled: boolean) => {
  if (!isTauri) {
    demoSettings = { ...demoSettings, stripExif: enabled };
    return Promise.resolve();
  }
  return invoke<void>("set_strip_exif", { enabled });
};

// ── pairing + quick text (M7) ────────────────────────────────────────────

// M7.1: mint a fresh pairing invitation (6-digit code + QR/deep-link payload),
// valid for 10 minutes. Calling it again replaces the previous code. Browser
// mode returns a static demo invite so the modal renders.
export const startPairing = () =>
  isTauri
    ? invoke<PairingInvite>("start_pairing")
    : Promise.resolve<PairingInvite>({
        code: "482913",
        qr: "lanbeam://pair?d=demo&n=%E4%B9%A6%E6%88%BF&a=192.168.1.20&p=51704&c=482913",
      });

// M7.1: cancel the active pairing code so it can no longer be redeemed. Safe
// to call when nothing is pairing.
export const cancelPairing = () =>
  isTauri ? invoke<void>("cancel_pairing") : Promise.resolve();

// A cold-start lanbeam://pair deep link the app was launched with, if any —
// pulled once on mount to open the pairing form for a link that arrived before
// the webview could listen. Null when launched normally (and in browser mode).
export const takePendingDeepLink = () =>
  isTauri
    ? invoke<string | null>("take_pending_deep_link")
    : Promise.resolve<string | null>(null);

// M7.1: join a device showing a pairing code. `addr` is its ip[:port] or a
// scanned lanbeam://pair link (which can itself carry the code). Resolves with
// the paired peer; a wrong/expired code rejects. Browser mode fakes a success.
export const joinByCode = (addr: string, code: string) =>
  isTauri
    ? invoke<ConnectResult>("join_by_code", { addr, code })
    : Promise.resolve<ConnectResult>({
        deviceId: "demo-paired",
        name: "Pixel 8 Pro",
        sas: "483921",
      });

// M7.2: connect to a device by ip[:port] (or a scanned link) so it appears on
// the devices page even when discovery can't see it. Grants no trust and sends
// nothing. Re-query listDiscoveredDevices after this resolves to pick it up.
export const connectByAddr = (addr: string) =>
  isTauri
    ? invoke<ConnectResult>("connect_by_addr", { addr })
    : Promise.resolve<ConnectResult>({
        deviceId: "demo-added",
        name: addr,
        sas: "483921",
      });

// M7.3: send a short text/link to a device. `alsoClipboard` asks the receiver
// to also place it on their clipboard — whether it lands there is the
// receiver's choice (their clipboard-sharing setting must allow it). Resolves
// once the peer confirms receipt; rejects on empty/oversized text, an unknown
// device, or a peer too old to receive text.
export const sendText = (
  deviceId: string,
  text: string,
  alsoClipboard: boolean,
) =>
  isTauri
    ? invoke<void>("send_text", { deviceId, text, alsoClipboard })
    : Promise.resolve();

// ── browser share (M8.2) ─────────────────────────────────────────────────

// A random demo token so browser mode's fake share link looks plausible; only
// reached without a backend (vite dev / design review), never in the app.
const demoShareToken = () =>
  (
    Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2)
  ).slice(0, 32);

// M8.2: publish files for a browser on the LAN to download — the fallback for a
// recipient without LanBeam. `ttlSecs` is the link lifetime, `maxDownloads` the
// download cap (null = unlimited). Resolves with the link, its token, and the
// expiry. Browser mode returns a demo localhost link so the modal renders.
export const startShare = (
  paths: string[],
  ttlSecs: number,
  maxDownloads: number | null,
) => {
  if (!isTauri) {
    const token = demoShareToken();
    return Promise.resolve<ShareStarted>({
      token,
      url: `http://127.0.0.1:51705/s/${token}`,
      expiresAt: Math.floor(Date.now() / 1000) + ttlSecs,
    });
  }
  return invoke<ShareStarted>("start_share", { paths, ttlSecs, maxDownloads });
};

// M8.2: reconfigure a live share's lifetime + download cap in place (the new
// lifetime starts now). Resolves with the new expiry, or null when the token is
// unknown / already stopped. Browser mode echoes a fresh expiry.
export const updateShare = (
  token: string,
  ttlSecs: number,
  maxDownloads: number | null,
) =>
  isTauri
    ? invoke<ShareUpdated | null>("update_share", {
        token,
        ttlSecs,
        maxDownloads,
      })
    : Promise.resolve<ShareUpdated>({
        expiresAt: Math.floor(Date.now() / 1000) + ttlSecs,
      });

// M8.2: stop a share now — its link dies immediately. Always safe (a no-op for
// an unknown token).
export const stopShare = (token: string) =>
  isTauri ? invoke<void>("stop_share", { token }) : Promise.resolve();

// M8.2: the browser shares currently live (link, file count, total size, expiry,
// downloads). Browser mode has no server, so it returns an empty list.
export const listShares = () =>
  isTauri
    ? invoke<ShareEntry[]>("list_shares")
    : Promise.resolve([] as ShareEntry[]);

// M5.7: factory-reset this device's identity; the backend restarts the app,
// so on success this promise never observably resolves.
export const resetIdentity = () =>
  isTauri
    ? invoke<void>("reset_identity")
    : Promise.reject(new Error("browser demo: no identity to reset"));

export const listDiscoveredDevices = () =>
  isTauri
    ? invoke<DiscoveredDevice[]>("list_discovered_devices")
    : Promise.resolve(DEMO_DEVICES);

// M2: open an authenticated Noise channel to a peer; returns the SAS.
export const connectDevice = (deviceId: string) =>
  isTauri
    ? invoke<string>("connect_device", { deviceId })
    : Promise.resolve("483921");

// M2: loopback self-test of the encrypted handshake; returns the SAS.

// M3: file transfer. `stripExif` (M9.1) removes photo metadata (location,
// camera, time) from JPEG/PNG/WebP images before sending — the per-send choice
// from the confirm dialog.
export const sendFiles = (
  deviceId: string,
  paths: string[],
  stripExif: boolean,
) =>
  isTauri
    ? invoke<string>("send_files", { deviceId, paths, stripExif })
    : Promise.reject(new Error("browser demo: no transport"));

// `conflict` (M6.5) is the ConflictModal's choice — "rename" (keep both) or
// "overwrite" — folded into the same reply as accept. Omitted for a bare
// accept/decline; the backend then falls back to the safe rename.
export const replyFileRequest = (
  sessionId: string,
  accept: boolean,
  conflict?: string,
) =>
  isTauri
    ? invoke<void>("reply_file_request", { sessionId, accept, conflict })
    : Promise.resolve();

// M6.1: cancel an in-flight transfer (either direction) — ends it promptly on
// both peers. Always safe: a no-op if the session id is unknown/finished.
export const cancelTransfer = (sessionId: string) =>
  isTauri ? invoke<void>("cancel_transfer", { sessionId }) : Promise.resolve();

// M6.2: pause an in-flight transfer — the loop stops moving bytes (TCP
// backpressure stalls the peer) until resumeTransfer. No-op if unknown.
export const pauseTransfer = (sessionId: string) =>
  isTauri ? invoke<void>("pause_transfer", { sessionId }) : Promise.resolve();

// M6.2: resume a transfer paused with pauseTransfer. No-op if unknown/not paused.
export const resumeTransfer = (sessionId: string) =>
  isTauri ? invoke<void>("resume_transfer", { sessionId }) : Promise.resolve();

// M6.4: discard the persisted resume state for a peer (forget every partial and
// delete the half-written files). The counterpart to letting a transfer resume.
/** One interrupted receive still holding bytes in the download folder. */
export type Partial = {
  deviceId: string;
  name: string;
  written: number;
  size: number;
};

/** Half-written files still on disk. They are saved under their FINAL name —
 *  `holiday.mp4`, 1.2 GB of 4 GB — so the download folder shows something that
 *  looks perfectly normal and plays for thirty seconds. The backend has always
 *  known about them (it is what makes resume work); nothing ever asked. */
export const listPartials = (): Promise<Partial[]> =>
  isTauri ? invoke<Partial[]>("list_partials") : Promise.resolve([]);

export const discardPartials = (deviceId: string) =>
  isTauri ? invoke<void>("discard_partials", { deviceId }) : Promise.resolve();

export const getDownloadDir = () =>
  isTauri
    ? invoke<string>("get_download_dir")
    : Promise.resolve("~/Downloads/LanBeam");

/** Open a path (a received file, or the download folder) with the OS default
 *  handler. Goes through the BACKEND rather than the opener plugin's JS
 *  `openPath`: that command is scope-gated, and the only static scope that would
 *  cover a user-relocatable download folder is "**" — a blanket "open any file
 *  on this machine" grant to the webview. Rejects with a `NotFound` error when
 *  the path is genuinely gone, so the UI can tell that apart from a failure. */
export const openLocalPath = (path: string) =>
  isTauri ? invoke<void>("open_local_path", { path }) : Promise.resolve();

export const revealReceived = (sessionId: string) =>
  isTauri
    ? invoke<string[]>("reveal_received", { sessionId })
    : Promise.resolve([]);

// M4.4: persistent trust store (backend-owned; `trust_updated` keeps the UI live)
export const listTrusted = () =>
  isTauri ? invoke<TrustedPeer[]>("list_trusted") : Promise.resolve([]);

// `pairedAt`/`lastSeen` (unix seconds) are only sent by the one-time
// localStorage migration so legacy records keep their original dates; the
// backend ignores them on updates, so regular calls must omit them.
export const setTrusted = (
  deviceId: string,
  name: string,
  autoAccept: boolean,
  pairedAt?: number,
  lastSeen?: number,
) =>
  isTauri
    ? invoke<void>("set_trusted", {
        deviceId,
        name,
        autoAccept,
        pairedAt,
        lastSeen,
      })
    : Promise.resolve();

export const removeTrusted = (deviceId: string) =>
  isTauri ? invoke<void>("remove_trusted", { deviceId }) : Promise.resolve();

/** Delete a device: its trust record AND the manually-added address (IP-direct /
 *  pair-by-code) that keeps it in the device list. Untrusting is `removeTrusted`
 *  — a device you stop trusting is still one you want to be able to reach. This
 *  is the stronger act, and it is what「删除设备」 has to call: `remove_trusted`
 *  alone left the peer sitting in the manual address table, back on the next
 *  list, with nothing anywhere able to take it out. */
export const forgetDevice = (deviceId: string) =>
  isTauri ? invoke<void>("forget_device", { deviceId }) : Promise.resolve();

// M4.6: degradations recorded at bind time. The matching `net_degraded`
// events fire during setup(), before the webview has any listener and Tauri
// events have no replay — so startup pulls the backlog once via this call.
export const getNetStatus = () =>
  isTauri
    ? invoke<NetDegradedEvent[]>("get_net_status")
    : Promise.resolve([] as NetDegradedEvent[]);

// M4.6: diagnostics — the app's log directory and a one-shot bundle export.
export const getLogDir = () =>
  isTauri ? invoke<string>("get_log_dir") : Promise.resolve("~/Logs/LanBeam");

export const exportDiagnostics = () =>
  isTauri
    ? invoke<string>("export_diagnostics")
    : Promise.resolve("~/Logs/LanBeam/lanbeam-diag-demo.txt");

/** Event subscription that is a no-op outside Tauri. Returns an unlisten fn. */
export function onEvent<T>(name: string, cb: (payload: T) => void): () => void {
  if (!isTauri) return () => {};
  const un: Promise<UnlistenFn> = listen<T>(name, (e) => cb(e.payload));
  return () => {
    un.then((off) => off());
  };
}
