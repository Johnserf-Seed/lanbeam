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
  beginSend,
  errText,
  isNotFound,
  newTransferId,
  openDir,
  openFile,
  readClipboardText,
  revealFile,
  sendTextTracked,
  sendToDevice,
  transferErrText,
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

  it("gives pairing failures their own words, not 'the connection failed'", () => {
    // These used to arrive as Protocol ("连接失败，请重试") or Io ("文件或磁盘出错")
    // — a wrong code and an unreachable peer both read as something else.
    const cases: [string, string][] = [
      ["PairRejected", "errors.pairRejected"],
      ["InvalidAddress", "errors.invalidAddress"],
      ["PairCodeMissing", "errors.pairCodeMissing"],
      ["Unreachable", "errors.unreachable"],
    ];
    for (const [kind, key] of cases) {
      const out = errText({ kind, message: "internal" });
      expect(out).toBe(i18n.t(key));
      expect(out).not.toBe(i18n.t("errors.protocol"));
      expect(out).not.toBe(i18n.t("errors.io"));
    }
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

describe("transferErrText", () => {
  it("says an undialable peer is unreachable, not a disk error", () => {
    expect(transferErrText("unreachable")).toBe(i18n.t("errors.unreachable"));
    expect(transferErrText("io")).toBe(i18n.t("errors.io"));
  });

  it("never shows a code it doesn't know — the generic line instead", () => {
    expect(transferErrText("brand_new")).toBe(i18n.t("errors.generic"));
    expect(transferErrText(undefined)).toBe(i18n.t("errors.generic"));
  });
});

describe("sendToDevice", () => {
  it("registers the outgoing transfer, records recents and invokes sendFiles", () => {
    vi.mocked(api.sendFiles).mockResolvedValue("session-abc");
    const files = [
      file(),
      file({ path: "/tmp/b.png", name: "b.png", ext: "PNG" }),
    ];

    const id = beginSend(device, files, true);

    // The UI chose the session id and keyed what it sent by it (H11).
    expect(id).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(useTransfers.getState().pendingSend[id as string]).toMatchObject({
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
      id,
    );
    // sendToDevice is the same thing, for callers that only need a yes/no.
    expect(sendToDevice(device, files, true)).toBe(true);
  });

  it("passes the explicit stripExif choice through to sendFiles", () => {
    vi.mocked(api.sendFiles).mockResolvedValue("s");
    sendToDevice(device, [file()], false);
    expect(api.sendFiles).toHaveBeenCalledWith(
      device.deviceId,
      ["/tmp/photo.jpg"],
      false,
      expect.any(String),
    );
  });

  it("a send that fails before any event leaves NO record for the next one (H11)", async () => {
    // The record used to sit in a per-device queue until the next sas_code —
    // the NEXT send to that device then wore this one's files and paths.
    vi.mocked(api.sendFiles).mockRejectedValueOnce({ kind: "PeerNotFound" });
    const failed = beginSend(device, [
      file({ name: "old.jpg", path: "/old.jpg" }),
    ]);
    await flush();
    expect(useTransfers.getState().pendingSend).toEqual({});

    vi.mocked(api.sendFiles).mockResolvedValue("s");
    const next = beginSend(device, [
      file({ name: "new.jpg", path: "/new.jpg" }),
    ]);
    expect(next).not.toBe(failed);
    useTransfers
      .getState()
      .attachSas(next as string, "111222", device.deviceId);
    expect(useTransfers.getState().transfers[next as string]).toMatchObject({
      name: "new.jpg",
      paths: ["/new.jpg"],
    });
  });

  it("returns false and toasts (no transfer) when no file has a path", () => {
    const ok = sendToDevice(device, [file({ path: undefined })], true);
    expect(ok).toBe(false);
    expect(api.sendFiles).not.toHaveBeenCalled();
    expect(useTransfers.getState().pendingSend).toEqual({});
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

describe("newTransferId", () => {
  it("is a fresh lowercase v4 UUID — what the backend accepts as an id", () => {
    const a = newTransferId();
    const b = newTransferId();
    expect(a).not.toBe(b);
    for (const id of [a, b])
      expect(id).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
      );
  });

  it("still makes one on a WebView without crypto.randomUUID", () => {
    const real = globalThis.crypto.randomUUID;
    Object.defineProperty(globalThis.crypto, "randomUUID", {
      value: undefined,
      configurable: true,
    });
    try {
      expect(newTransferId()).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
      );
    } finally {
      Object.defineProperty(globalThis.crypto, "randomUUID", {
        value: real,
        configurable: true,
      });
    }
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
