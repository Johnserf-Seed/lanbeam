// Unit tests for the shared send/open operations. The bridge is stubbed so we
// can exercise the Tauri-mode orchestration (register a transfer, add recents,
// call sendFiles / open the system opener) without a live backend.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import type { SendFile } from "./store";

// Keep the real module (store + other pages rely on its exports) but force
// Tauri mode on and replace the two invoke-backed commands with spies.
vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return {
    ...actual,
    isTauri: true,
    openLocalPath: vi.fn().mockResolvedValue(undefined),
    sendFiles: vi.fn(),
    sendText: vi.fn(),
  };
});

// The dynamic `import("@tauri-apps/plugin-opener")` inside openDir/revealFile
// resolves to these spies, so no real OS call is attempted.
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn().mockResolvedValue(undefined),
  revealItemInDir: vi.fn().mockResolvedValue(undefined),
}));

// Same for the clipboard plugin the (isTauri) paste path reads through.
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  readText: vi.fn().mockResolvedValue(""),
}));

import * as api from "../bridge/api";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import {
  errText,
  isNotFound,
  openDir,
  openFile,
  readClipboardText,
  revealFile,
  sendTextTracked,
  sendToDevice,
} from "./sendops";
import { useRecents, useToast, useTransfers } from "./store";
import i18n from "../i18n";

const flush = () => new Promise((r) => setTimeout(r, 0));

const device: DiscoveredDevice = {
  deviceId: "peer-1",
  name: "Living Room",
  address: "192.168.1.5",
  port: 51704,
};

const file = (over: Partial<SendFile> = {}): SendFile => ({
  path: "/tmp/photo.jpg",
  name: "photo.jpg",
  ext: "JPG",
  size: 1234,
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  useTransfers.setState({ transfers: {}, pendingSend: {} });
  useRecents.setState({ items: [] });
  useToast.setState({ msg: null, action: null });
});

describe("errText", () => {
  it("returns a bare string unchanged", () => {
    expect(errText("boom")).toBe("boom");
  });

  it("maps a known error kind to its localized message (never the raw tag)", () => {
    expect(errText({ kind: "PeerTooOld" })).toBe(i18n.t("errors.peerTooOld"));
    expect(errText({ kind: "PeerNotFound" })).toBe(
      i18n.t("errors.peerNotFound"),
    );
    expect(errText({ kind: "Timeout" })).toBe(i18n.t("errors.timeout"));
    expect(errText({ kind: "PeerTooOld" })).not.toBe("PeerTooOld");
  });

  it("uses the kind and NEVER surfaces the internal English message", () => {
    // `message` is internal diagnostics (error.rs) — it must not reach the UI.
    const out = errText({
      kind: "Protocol",
      message: "not a valid address: 307246",
    });
    expect(out).toBe(i18n.t("errors.protocol"));
    expect(out).not.toContain("307246");
    expect(out).not.toContain("not a valid address");
  });

  it("falls back to a generic localized message for unknown/absent kinds", () => {
    const generic = i18n.t("errors.generic");
    expect(errText({ kind: "SomethingNew" })).toBe(generic);
    expect(errText({ message: "oops" })).toBe(generic); // a message with no kind
    expect(errText(new Error("bad thing"))).toBe(generic);
    expect(errText(null)).toBe(generic);
    expect(errText(undefined)).toBe(generic);
    expect(errText(42)).toBe(generic);
    expect(errText({})).toBe(generic);
  });
});

describe("sendToDevice", () => {
  it("registers the outgoing transfer, records recents and invokes sendFiles", () => {
    vi.mocked(api.sendFiles).mockResolvedValue("session-abc");
    const files = [
      file(),
      file({ path: "/tmp/b.png", name: "b.png", ext: "PNG" }),
    ];

    const ok = sendToDevice(device, files, true);

    expect(ok).toBe(true);
    const queue = useTransfers.getState().pendingSend[device.deviceId];
    expect(queue).toHaveLength(1);
    expect(queue[0]).toMatchObject({
      peerId: device.deviceId,
      peerName: device.name,
      name: "photo.jpg",
      paths: ["/tmp/photo.jpg", "/tmp/b.png"],
    });
    expect(useRecents.getState().items).toHaveLength(2);
    expect(api.sendFiles).toHaveBeenCalledWith(
      device.deviceId,
      ["/tmp/photo.jpg", "/tmp/b.png"],
      true,
    );
  });

  it("passes the explicit stripExif choice through to sendFiles", () => {
    vi.mocked(api.sendFiles).mockResolvedValue("s");
    sendToDevice(device, [file()], false);
    expect(api.sendFiles).toHaveBeenCalledWith(
      device.deviceId,
      ["/tmp/photo.jpg"],
      false,
    );
  });

  it("returns false and toasts (no transfer) when no file has a path", () => {
    const ok = sendToDevice(device, [file({ path: undefined })], true);
    expect(ok).toBe(false);
    expect(api.sendFiles).not.toHaveBeenCalled();
    expect(
      useTransfers.getState().pendingSend[device.deviceId],
    ).toBeUndefined();
    expect(useToast.getState().msg).toBeTruthy();
  });

  it("toasts on a generic send failure", async () => {
    vi.mocked(api.sendFiles).mockRejectedValue(new Error("network down"));
    sendToDevice(device, [file()], true);
    await flush();
    expect(useToast.getState().msg).toBeTruthy();
  });

  it("stays silent on a Rejected failure (a decline is narrated elsewhere)", async () => {
    vi.mocked(api.sendFiles).mockRejectedValue({ kind: "Rejected" });
    sendToDevice(device, [file()], true);
    await flush();
    expect(useToast.getState().msg).toBeNull();
  });
});

describe("sendTextTracked", () => {
  it("sends the text and records a history entry on success", async () => {
    vi.mocked(api.sendText).mockResolvedValue(undefined);

    await sendTextTracked("peer-1", "Living Room", "hello there", true);

    expect(api.sendText).toHaveBeenCalledWith("peer-1", "hello there", true);
    const recs = Object.values(useTransfers.getState().transfers);
    expect(recs).toHaveLength(1);
    expect(recs[0]).toMatchObject({
      kind: "text",
      direction: "send",
      peerId: "peer-1",
      peerName: "Living Room",
      text: "hello there",
    });
  });

  it("rethrows and records nothing when the send rejects", async () => {
    vi.mocked(api.sendText).mockRejectedValue(new Error("too old"));

    await expect(
      sendTextTracked("peer-1", "Living Room", "hi", false),
    ).rejects.toThrow("too old");
    expect(Object.values(useTransfers.getState().transfers)).toHaveLength(0);
  });
});

describe("openDir / openFile / revealFile", () => {
  // REGRESSION GUARD. Opening used to go through the opener plugin's JS
  // `openPath`, which is SCOPE-GATED — and the capability never granted it, so
  // "open file" / "open download folder" were dead in every build. They now go
  // through the backend (`open_local_path`), whose Rust api needs no such
  // scope. If anyone routes them back through the plugin, these fail.
  it("opens a directory through the BACKEND, not the scope-gated plugin", async () => {
    await openDir("/tmp/downloads");
    expect(api.openLocalPath).toHaveBeenCalledWith("/tmp/downloads");
    expect(openPath).not.toHaveBeenCalled();
  });

  it("opens a file through the BACKEND, not the scope-gated plugin", async () => {
    await openFile("/tmp/downloads/photo.jpg");
    expect(api.openLocalPath).toHaveBeenCalledWith("/tmp/downloads/photo.jpg");
    expect(openPath).not.toHaveBeenCalled();
  });

  it("still reveals through the plugin (reveal IS granted by the capability)", async () => {
    await revealFile("/tmp/downloads/photo.jpg");
    expect(revealItemInDir).toHaveBeenCalledWith("/tmp/downloads/photo.jpg");
  });
});

describe("isNotFound", () => {
  it("only reports true for the backend's NotFound kind", () => {
    // This is what lets the inbox say "that file is gone" ONLY when it really
    // is — instead of blaming a stale record for every failure, which is how
    // the scope-gate bug stayed hidden.
    expect(isNotFound({ kind: "NotFound", message: "x" })).toBe(true);
    expect(isNotFound({ kind: "Io", message: "x" })).toBe(false);
    expect(isNotFound(new Error("boom"))).toBe(false);
    expect(isNotFound(null)).toBe(false);
  });
});

describe("readClipboardText", () => {
  it("reads via the Tauri clipboard plugin in app mode (no DOM prompt)", async () => {
    vi.mocked(readText).mockResolvedValue("clip contents");
    // isTauri is true (mocked at top), so the plugin path is taken — the
    // Rust-backed read that avoids the WebView permission prompt.
    await expect(readClipboardText()).resolves.toBe("clip contents");
    expect(readText).toHaveBeenCalled();
  });

  it("returns an empty string when the clipboard read fails or has no text", async () => {
    vi.mocked(readText).mockRejectedValue(new Error("no text on clipboard"));
    await expect(readClipboardText()).resolves.toBe("");
  });
});
