// Browser-mode (non-Tauri) contract tests for the bridge wrappers. In the test
// env `isTauri` is false, so every command resolves its static demo stub with
// no IPC — these assert those stubbed shapes and the onEvent no-op.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cancelPairing,
  cancelTransfer,
  connectByAddr,
  connectDevice,
  discardPartials,
  exportDiagnostics,
  getDownloadDir,
  getListenPort,
  getLogDir,
  getMyIdentity,
  getNetStatus,
  getNetworkInfo,
  getSettings,
  isTauri,
  joinByCode,
  listDiscoveredDevices,
  listShares,
  listTrusted,
  onEvent,
  pauseTransfer,
  removeTrusted,
  replyFileRequest,
  resetIdentity,
  resumeTransfer,
  revealReceived,
  sendFiles,
  sendText,
  setAutoOpen,
  setAutostart,
  setClipShare,
  setConflictPolicy,
  setDeviceName,
  setDiscoverable,
  setDownloadDir,
  setHotkey,
  setHotkeyEnabled,
  setIfaceFilter,
  setListenPort,
  setLogLevel,
  setMaxConcurrent,
  setNotifSystem,
  setOrganize,
  setRateLimit,
  setRecvPolicy,
  setStripExif,
  setTrayClose,
  setTrusted,
  setVerifyHash,
  startPairing,
  startShare,
  stopShare,
  takePendingDeepLink,
  updateShare,
} from "./api";

describe("isTauri", () => {
  it("is false in the happy-dom test env (no __TAURI_INTERNALS__)", () => {
    expect(isTauri).toBe(false);
  });
});

describe("identity + settings stubs", () => {
  it("getMyIdentity returns the demo identity with the current device name", async () => {
    const id = await getMyIdentity();
    expect(id.deviceId).toHaveLength(43);
    expect(id.shortId).toBe(id.deviceId.slice(0, 8));
    expect(typeof id.name).toBe("string");
  });

  it("getSettings returns a fresh copy each call (not the same reference)", async () => {
    const a = await getSettings();
    const b = await getSettings();
    expect(a).not.toBe(b);
    expect(a).toEqual(b);
    expect(a).toMatchObject({
      discoverable: expect.any(Boolean),
      logLevel: expect.any(String),
      port: expect.any(Number),
      maxConcurrent: expect.any(Number),
    });
  });

  it("setDeviceName mutates the demo settings and is reflected by later reads", async () => {
    await setDeviceName("测试设备");
    const id = await getMyIdentity();
    const s = await getSettings();
    expect(id.name).toBe("测试设备");
    expect(s.deviceName).toBe("测试设备");
  });
});

describe("boolean/scalar setters resolve void and persist to demo settings", () => {
  // Each entry: [setter, arg, settings key, expected stored value]
  const cases: Array<[(v: never) => Promise<void>, unknown, string, unknown]> =
    [
      [setDiscoverable as never, false, "discoverable", false],
      [setAutoOpen as never, true, "autoOpenFolder", true],
      [setLogLevel as never, "verbose", "logLevel", "verbose"],
      [setRecvPolicy as never, "all", "recvPolicy", "all"],
      [setListenPort as never, 51999, "port", 51999],
      [setTrayClose as never, false, "trayClose", false],
      [setNotifSystem as never, false, "notifSystem", false],
      [setAutostart as never, true, "autostart", true],
      [setHotkeyEnabled as never, true, "hotkeyEnabled", true],
      [setHotkey as never, "Ctrl+Shift+K", "hotkey", "Ctrl+Shift+K"],
      [setVerifyHash as never, false, "verifyHash", false],
      [setConflictPolicy as never, "overwrite", "conflict", "overwrite"],
      [setOrganize as never, "date", "organize", "date"],
      [setMaxConcurrent as never, 5, "maxConcurrent", 5],
      [setRateLimit as never, "10", "rateLimit", "10"],
      [setClipShare as never, true, "clipShare", true],
      [setStripExif as never, false, "stripExif", false],
    ];

  for (const [setter, arg, key, expected] of cases) {
    it(`${key} setter resolves and stores ${String(expected)}`, async () => {
      await expect(setter(arg as never)).resolves.toBeUndefined();
      const s = await getSettings();
      expect((s as Record<string, unknown>)[key]).toBe(expected);
    });
  }

  it("setIfaceFilter stores a non-empty ip and clears to undefined on empty", async () => {
    await setIfaceFilter("192.168.1.20");
    expect((await getSettings()).ifaceFilter).toBe("192.168.1.20");
    await setIfaceFilter("");
    expect((await getSettings()).ifaceFilter).toBeUndefined();
  });
});

describe("network + download dir stubs", () => {
  it("getNetworkInfo returns the demo IPv4 endpoint list", async () => {
    const net = await getNetworkInfo();
    expect(net).toHaveLength(1);
    expect(net[0]).toEqual({ ip: "192.168.1.20", broadcast: "192.168.1.255" });
  });

  it("setDownloadDir echoes the path it was given", async () => {
    await expect(setDownloadDir("/tmp/x")).resolves.toBe("/tmp/x");
  });

  it("getDownloadDir returns the demo default folder", async () => {
    await expect(getDownloadDir()).resolves.toBe("~/Downloads/LanBeam");
  });

  it("getListenPort echoes the configured port, or the 51704 default when 0", async () => {
    await setListenPort(0);
    await expect(getListenPort()).resolves.toBe(51704);
    await setListenPort(52000);
    await expect(getListenPort()).resolves.toBe(52000);
  });
});

describe("pairing + quick text stubs", () => {
  it("startPairing returns the static demo invite (code + qr deep link)", async () => {
    const invite = await startPairing();
    expect(invite.code).toBe("482913");
    expect(invite.qr).toMatch(/^lanbeam:\/\/pair\?/);
    expect(invite.qr).toContain("c=482913");
  });

  it("cancelPairing resolves void", async () => {
    await expect(cancelPairing()).resolves.toBeUndefined();
  });

  it("takePendingDeepLink resolves null in browser mode", async () => {
    await expect(takePendingDeepLink()).resolves.toBeNull();
  });

  it("joinByCode fakes a paired peer regardless of args", async () => {
    const res = await joinByCode("192.168.1.9:51704", "000000");
    expect(res).toEqual({
      deviceId: "demo-paired",
      name: "Pixel 8 Pro",
      sas: "483921",
    });
  });

  it("connectByAddr echoes the address as the peer name", async () => {
    const res = await connectByAddr("192.168.1.42");
    expect(res).toEqual({
      deviceId: "demo-added",
      name: "192.168.1.42",
      sas: "483921",
    });
  });

  it("sendText resolves void", async () => {
    await expect(
      sendText("demo-mini", "hello", false),
    ).resolves.toBeUndefined();
  });
});

describe("browser share stubs", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-13T00:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("startShare returns a demo localhost link with an alphanumeric token and ttl expiry", async () => {
    const nowSecs = Math.floor(Date.now() / 1000);
    const share = await startShare(["/a", "/b"], 600, null);
    expect(share.token).toMatch(/^[a-z0-9]+$/);
    expect(share.token.length).toBeLessThanOrEqual(32);
    expect(share.token.length).toBeGreaterThan(0);
    expect(share.url).toBe(`http://127.0.0.1:51705/s/${share.token}`);
    expect(share.expiresAt).toBe(nowSecs + 600);
  });

  it("updateShare echoes a fresh expiry from now plus ttl", async () => {
    const nowSecs = Math.floor(Date.now() / 1000);
    const res = await updateShare("tok", 300, 5);
    expect(res).toEqual({ expiresAt: nowSecs + 300 });
  });

  it("stopShare resolves void", async () => {
    await expect(stopShare("tok")).resolves.toBeUndefined();
  });

  it("listShares returns an empty list (no share server in browser mode)", async () => {
    await expect(listShares()).resolves.toEqual([]);
  });
});

describe("discovery + secure channel stubs", () => {
  it("listDiscoveredDevices returns the 5 demo devices", async () => {
    const devices = await listDiscoveredDevices();
    expect(devices).toHaveLength(5);
    expect(devices[0]).toMatchObject({
      deviceId: "demo-mini",
      name: "客厅 · Mac mini",
      port: 52637,
    });
    for (const d of devices) {
      expect(d.address).toMatch(/^192\.168\.1\./);
    }
  });

  it("connectDevice resolves the demo SAS", async () => {
    await expect(connectDevice("demo-mini")).resolves.toBe("483921");
  });
});

describe("transfer control stubs", () => {
  it("replyFileRequest resolves void (with and without a conflict choice)", async () => {
    await expect(replyFileRequest("s1", true)).resolves.toBeUndefined();
    await expect(
      replyFileRequest("s1", true, "rename"),
    ).resolves.toBeUndefined();
  });

  it("cancel/pause/resume/discard all resolve void", async () => {
    await expect(cancelTransfer("s1")).resolves.toBeUndefined();
    await expect(pauseTransfer("s1")).resolves.toBeUndefined();
    await expect(resumeTransfer("s1")).resolves.toBeUndefined();
    await expect(discardPartials("demo-mini")).resolves.toBeUndefined();
  });

  it("revealReceived returns an empty list", async () => {
    await expect(revealReceived("s1")).resolves.toEqual([]);
  });
});

describe("rejecting stubs (no transport in browser mode)", () => {
  it("sendFiles rejects", async () => {
    await expect(sendFiles("demo-mini", ["/a"], false)).rejects.toThrow(
      /no transport/,
    );
  });

  it("resetIdentity rejects", async () => {
    await expect(resetIdentity()).rejects.toThrow(/no identity/);
  });
});

describe("trust store stubs", () => {
  it("listTrusted returns an empty list", async () => {
    await expect(listTrusted()).resolves.toEqual([]);
  });

  it("setTrusted resolves void (with and without legacy timestamps)", async () => {
    await expect(setTrusted("d", "n", true)).resolves.toBeUndefined();
    await expect(setTrusted("d", "n", false, 1, 2)).resolves.toBeUndefined();
  });

  it("removeTrusted resolves void", async () => {
    await expect(removeTrusted("d")).resolves.toBeUndefined();
  });
});

describe("diagnostics stubs", () => {
  it("getNetStatus returns an empty list", async () => {
    await expect(getNetStatus()).resolves.toEqual([]);
  });

  it("getLogDir returns the demo log directory", async () => {
    await expect(getLogDir()).resolves.toBe("~/Logs/LanBeam");
  });

  it("exportDiagnostics returns the demo bundle path", async () => {
    await expect(exportDiagnostics()).resolves.toBe(
      "~/Logs/LanBeam/lanbeam-diag-demo.txt",
    );
  });
});

describe("onEvent", () => {
  it("returns a no-op unlisten fn that is safe to call and never invokes the callback", () => {
    const cb = vi.fn();
    const unlisten = onEvent<{ x: number }>("pair_joined", cb);
    expect(typeof unlisten).toBe("function");
    expect(() => unlisten()).not.toThrow();
    expect(cb).not.toHaveBeenCalled();
  });
});
