/** Shared send/open operations used by pages, modals and drag-drop. */
import i18n from "../i18n";
import * as api from "../bridge/api";
import type { DiscoveredDevice } from "../bridge/api";
import { extOf } from "./format";
import {
  sendFileFromPath,
  showToast,
  useData,
  useOverlays,
  usePrefs,
  useRecents,
  useTransfers,
  type SendFile,
  type UITransfer,
} from "./store";

/** Kick off a real transfer to one device. Resolution of `send_files` is the
 *  whole transfer, so this fires and reports failures via toast + events.
 *  `stripExif` (M9.1) is the per-send metadata-strip choice; it defaults to the
 *  persisted preference so the direct-drop / resend / forward entrypoints match
 *  the confirm dialog's default, and the dialog passes its live checkbox value. */
export function sendToDevice(
  d: DiscoveredDevice,
  files: SendFile[],
  stripExif = usePrefs.getState().stripExif,
): boolean {
  const paths = files.flatMap((f) => (f.path ? [f.path] : []));
  if (!paths.length || !api.isTauri) {
    showToast(i18n.t("common.milestoneNote"));
    return false;
  }
  useTransfers.getState().registerOutgoing(d.deviceId, {
    peerId: d.deviceId,
    peerName: d.name,
    name: files[0].name,
    ext: extOf(files[0].name),
    files: files.map((f) => ({ name: f.name, size: f.size })),
    paths,
  });
  useRecents.getState().add(files);
  api.sendFiles(d.deviceId, paths, stripExif).catch((e) => {
    // A decline is already narrated by the transfer_error handler (M4.5
    // code "declined") — a second generic failure toast would clobber it.
    if ((e as { kind?: string } | null)?.kind !== "Rejected") {
      showToast(
        i18n.t("send.sendFailedToast", {
          name: d.name,
          err: errText(e),
        }),
      );
    }
    // Pre-handshake failures never produce a matching transfer, so flag the
    // waiting row directly — otherwise it blinks "waiting" forever.
    const cur = useOverlays.getState().send;
    if (cur && cur.step === "waiting") {
      useOverlays.getState().patchSend({
        pending: cur.pending.map((p) =>
          p.deviceId === d.deviceId ? { ...p, failed: errText(e) } : p,
        ),
      });
    }
  });
  return true;
}

/** Send a quick text over the encrypted channel AND record it in the transfer
 *  history as a lightweight "text" entry (M7.3). Awaits the send; on success
 *  stamps a sent-text record so「everything I sent」shows up in 历史. Rethrows on
 *  failure so callers can toast. In browser-demo mode api.sendText resolves as a
 *  no-op, so the record is still written there without crashing. */
export async function sendTextTracked(
  deviceId: string,
  deviceName: string,
  text: string,
  clip: boolean,
): Promise<void> {
  await api.sendText(deviceId, text, clip);
  useTransfers.getState().addTextTransfer({
    direction: "send",
    peerId: deviceId,
    peerName: deviceName,
    text,
  });
}

/** Backend error `kind` (the stable serde tag on LanBeamError) → i18n key. The
 *  error's inner `message` is INTERNAL diagnostics (see error.rs) and must never
 *  reach the UI — only the localized text for its `kind` does. */
const ERR_KEYS: Record<string, string> = {
  PeerTooOld: "errors.peerTooOld",
  PeerNotFound: "errors.peerNotFound",
  Protocol: "errors.protocol",
  Handshake: "errors.handshake",
  IdentityMismatch: "errors.identityMismatch",
  UnsafePath: "errors.unsafePath",
  Keyring: "errors.keyring",
  Crypto: "errors.crypto",
  Rejected: "errors.rejected",
  Cancelled: "errors.cancelled",
  Timeout: "errors.timeout",
  Integrity: "errors.integrity",
  Io: "errors.io",
  Bind: "errors.io",
};

/** Localized, user-facing text for a caught error. Driven by the error's `kind`
 *  (the stable contract); the raw English `message` is deliberately NOT shown.
 *  A bare string passes through (already human-readable); an unknown or absent
 *  kind falls back to a generic localized message. */
export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  const kind = (e as { kind?: string } | null)?.kind;
  return i18n.t(kind && ERR_KEYS[kind] ? ERR_KEYS[kind] : "errors.generic");
}

/** Open a received file with the system default app. */
export async function openFile(path: string): Promise<void> {
  if (!api.isTauri) {
    showToast(i18n.t("common.milestoneNote"));
    return;
  }
  const { openPath } = await import("@tauri-apps/plugin-opener");
  await openPath(path);
}

/** Open a directory itself in the system file manager (revealFile would open
 *  its PARENT with the directory merely selected). */
export async function openDir(path: string): Promise<void> {
  if (!api.isTauri) {
    showToast(i18n.t("common.milestoneNote"));
    return;
  }
  const { openPath } = await import("@tauri-apps/plugin-opener");
  await openPath(path);
}

/** Reveal a received file in the system file manager. */
export async function revealFile(path: string): Promise<void> {
  if (!api.isTauri) {
    showToast(i18n.t("common.milestoneNote"));
    return;
  }
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/** Pick files with the native dialog; browser mode returns []. */
export async function pickFiles(title: string): Promise<SendFile[]> {
  if (!api.isTauri) {
    showToast(i18n.t("common.milestoneNote"));
    return [];
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const sel = await open({ multiple: true, title });
  if (!sel) return [];
  const paths = Array.isArray(sel) ? sel : [sel];
  return paths.map(sendFileFromPath);
}

/** Best-effort copy to the system clipboard. The (rare) rejection is swallowed;
 *  callers pair this with their own confirmation toast. One implementation so the
 *  swallowed-rejection handling lives in a single place. */
export function copyText(text: string): void {
  try {
    void navigator.clipboard?.writeText(text);
  } catch {
    /* clipboard unavailable */
  }
}

/** Read the system clipboard as text. In the packaged app this goes through the
 *  Tauri clipboard plugin (the Rust core reads the OS clipboard directly), so the
 *  WebView never shows its own "site wants to see your clipboard" PERMISSION
 *  prompt — the one a DOM `navigator.clipboard.readText()` pops on first paste.
 *  Browser/demo mode falls back to the DOM API (a prompt there is acceptable, and
 *  outside Tauri there is no alternative). Returns "" on any failure — no text on
 *  the clipboard, a non-text payload, or a denied read — so callers can no-op. */
export async function readClipboardText(): Promise<string> {
  try {
    if (api.isTauri) {
      const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
      return await readText();
    }
    return await navigator.clipboard.readText();
  } catch {
    return "";
  }
}

/** Retry a failed outgoing transfer to the same device. Returns whether it
 *  actually started (false = device offline / not resendable) so a caller can,
 *  e.g., close the detail drawer only on success. */
export function resendTransfer(tr: UITransfer): boolean {
  const dev = useData.getState().devices.find((d) => d.deviceId === tr.peerId);
  if (!dev) {
    showToast(i18n.t("devices.offlineToast"));
    return false;
  }
  return sendToDevice(dev, (tr.paths ?? []).map(sendFileFromPath));
}
