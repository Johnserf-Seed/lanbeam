// Unit tests for the zustand stores + pure helpers in store.ts. The test env is
// non-Tauri (api.isTauri === false), so load()/trust writes take their
// browser-mode branch (demo data / no IPC). Stores are reset to a captured
// snapshot before each test so cases can't leak into one another.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  DiscoveredDevice,
  IncomingRequest,
  TrustedPeer,
} from "../bridge/api";
import {
  displayIp,
  histWindowMs,
  inboxFromText,
  inboxFromTransfer,
  resolvedTheme,
  sendFileFromPath,
  shortFp,
  transferList,
  trustList,
  useData,
  useInbox,
  useOverlays,
  usePrefs,
  useRecents,
  useSysDark,
  useToast,
  useTransfers,
  useTrust,
  visibilityOf,
} from "./store";
import type { OutgoingMeta, TrustRecord, UITransfer } from "./store";

// ── snapshots for reset ────────────────────────────────────────────────────
const prefs0 = { ...usePrefs.getState() };
const data0 = { ...useData.getState() };
const transfers0 = { ...useTransfers.getState() };
const trust0 = { ...useTrust.getState() };
const inbox0 = { ...useInbox.getState() };
const recents0 = { ...useRecents.getState() };
const toast0 = { ...useToast.getState() };
const overlays0 = { ...useOverlays.getState() };
const sysDark0 = { ...useSysDark.getState() };

beforeEach(() => {
  usePrefs.setState(prefs0, true);
  useData.setState(data0, true);
  useTransfers.setState(transfers0, true);
  useTrust.setState(trust0, true);
  useInbox.setState(inbox0, true);
  useRecents.setState(recents0, true);
  useToast.setState(toast0, true);
  useOverlays.setState(overlays0, true);
  useSysDark.setState(sysDark0, true);
});

afterEach(() => {
  vi.useRealTimers();
});

// ── pure helpers ────────────────────────────────────────────────────────────
describe("resolvedTheme", () => {
  it("returns the explicit mode unchanged", () => {
    expect(resolvedTheme("light", true)).toBe("light");
    expect(resolvedTheme("dark", false)).toBe("dark");
  });
  it("resolves system to the OS preference", () => {
    expect(resolvedTheme("system", true)).toBe("dark");
    expect(resolvedTheme("system", false)).toBe("light");
  });
});

describe("shortFp", () => {
  it("strips punctuation, uppercases, and groups 4·4", () => {
    expect(shortFp("demo-mini")).toBe("DEMO · MINI");
  });
  it("handles a long base64url id", () => {
    expect(shortFp("vJx0Qm8dR3ke")).toBe("VJX0 · QM8D");
  });
});

describe("visibilityOf", () => {
  const on = { discoverable: true } as never;
  const off = { discoverable: false } as never;
  it("is 'on' whenever the backend flag is set", () => {
    expect(visibilityOf(on, null, 100)).toBe("on");
    expect(visibilityOf(on, 999, 100)).toBe("on");
  });
  it("is 'ghost' while the timer is still in the future", () => {
    expect(visibilityOf(off, 500, 100)).toBe("ghost");
  });
  it("is 'off' when hidden with no live ghost timer", () => {
    expect(visibilityOf(off, null, 100)).toBe("off");
    expect(visibilityOf(off, 50, 100)).toBe("off");
    expect(visibilityOf(null, null, 100)).toBe("off");
  });
});

describe("displayIp", () => {
  const info = [
    { ip: "10.0.0.1", broadcast: null },
    { ip: "192.168.1.5", broadcast: null },
  ];
  it("prefers the iface filter when it matches a live interface", () => {
    expect(displayIp(info, "192.168.1.5")).toBe("192.168.1.5");
  });
  it("falls back to the first entry when the filter doesn't resolve", () => {
    expect(displayIp(info, "8.8.8.8")).toBe("10.0.0.1");
    expect(displayIp(info, "")).toBe("10.0.0.1");
  });
  it("is undefined when there are no interfaces", () => {
    expect(displayIp([], "")).toBeUndefined();
  });
});

describe("sendFileFromPath", () => {
  it("derives name + ext from a posix path", () => {
    expect(sendFileFromPath("/home/me/photo.png")).toEqual({
      path: "/home/me/photo.png",
      name: "photo.png",
      ext: "PNG",
    });
  });
  it("handles a windows path with backslashes", () => {
    const f = sendFileFromPath("C:\\Users\\me\\report.docx");
    expect(f.name).toBe("report.docx");
    expect(f.ext).toBe("DOCX");
  });
});

describe("transferList", () => {
  it("orders by startedAt descending", () => {
    const map: Record<string, UITransfer> = {
      a: { sessionId: "a", startedAt: 100 } as UITransfer,
      b: { sessionId: "b", startedAt: 300 } as UITransfer,
      c: { sessionId: "c", startedAt: 200 } as UITransfer,
    };
    expect(transferList(map).map((t) => t.sessionId)).toEqual(["b", "c", "a"]);
  });
});

describe("useTransfers.addShareDownload", () => {
  beforeEach(() => useTransfers.setState({ transfers: {} }));

  it("records a browser download as a done 'send' history entry", () => {
    useTransfers.getState().addShareDownload({
      name: "report.pdf",
      size: 2048,
      peerIp: "192.168.1.42",
    });
    const recs = Object.values(useTransfers.getState().transfers);
    expect(recs).toHaveLength(1);
    expect(recs[0]).toMatchObject({
      kind: "file",
      via: "browser",
      direction: "send",
      peerName: "192.168.1.42", // the downloader's IP stands in for a peer
      name: "report.pdf",
      ext: "PDF",
      totalSize: 2048,
      status: "done",
      percent: 100,
    });
  });

  it("mints a unique session id per download (no collisions)", () => {
    const { addShareDownload } = useTransfers.getState();
    addShareDownload({ name: "a.png", size: 1, peerIp: "10.0.0.1" });
    addShareDownload({ name: "a.png", size: 1, peerIp: "10.0.0.1" });
    expect(Object.keys(useTransfers.getState().transfers)).toHaveLength(2);
  });
});

describe("trustList", () => {
  const dev = (id: string, name: string): DiscoveredDevice => ({
    deviceId: id,
    name,
    address: "1.1.1.1",
    port: 1,
  });
  const rec = (over: Partial<TrustRecord>): TrustRecord => ({
    deviceId: "x",
    name: "x",
    trusted: false,
    autoAccept: false,
    addedAt: 1,
    lastSeen: 1,
    ...over,
  });

  it("marks live devices online and folds in their trust record", () => {
    const out = trustList([dev("d1", "Bob")], {
      d1: rec({
        deviceId: "d1",
        name: "BobRenamed",
        trusted: true,
        autoAccept: true,
      }),
    });
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({
      deviceId: "d1",
      name: "BobRenamed",
      online: true,
      trusted: true,
      autoAccept: true,
    });
  });

  it("lists remembered records that aren't live as offline", () => {
    const out = trustList([], {
      d9: rec({ deviceId: "d9", name: "Ghost", trusted: true }),
    });
    expect(out).toHaveLength(1);
    expect(out[0].online).toBe(false);
  });

  it("flags a fingerprint change when the same name reappears under a new key", () => {
    const out = trustList([dev("newkey", "Alice")], {
      oldkey: rec({ deviceId: "oldkey", name: "Alice", trusted: true }),
    });
    const offline = out.find((d) => !d.online);
    expect(offline?.fpChanged).toEqual({ newDeviceId: "newkey" });
  });
});

// ── usePrefs ─────────────────────────────────────────────────────────────────
describe("usePrefs", () => {
  it("has the documented defaults", () => {
    const p = usePrefs.getState();
    expect(p.themeMode).toBe("light");
    expect(p.ghostUntil).toBeNull();
    expect(p.port).toBe("51704");
    expect(p.soundKind).toBe("叮咚");
    expect(p.verifyHash).toBe(true);
  });
  it("merges a partial via set", () => {
    usePrefs.getState().set({ themeMode: "dark", logLevel: "verbose" });
    expect(usePrefs.getState().themeMode).toBe("dark");
    expect(usePrefs.getState().logLevel).toBe("verbose");
    // untouched keys survive
    expect(usePrefs.getState().port).toBe("51704");
  });
});

describe("useSysDark", () => {
  it("updates the dark flag", () => {
    useSysDark.getState().set(true);
    expect(useSysDark.getState().dark).toBe(true);
    expect(resolvedTheme("system", useSysDark.getState().dark)).toBe("dark");
  });
});

// ── useData ──────────────────────────────────────────────────────────────────
describe("useData.setDevices", () => {
  it("stamps firstSeen once and preserves it across re-discovery", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1000);
    useData
      .getState()
      .setDevices([{ deviceId: "a", name: "A", address: "", port: 0 }]);
    expect(useData.getState().firstSeen.a).toBe(1000);

    vi.setSystemTime(5000);
    useData.getState().setDevices([
      { deviceId: "a", name: "A", address: "", port: 0 },
      { deviceId: "b", name: "B", address: "", port: 0 },
    ]);
    const { firstSeen, devices } = useData.getState();
    expect(firstSeen.a).toBe(1000); // unchanged
    expect(firstSeen.b).toBe(5000); // fresh
    expect(devices).toHaveLength(2);
  });
});

describe("useData.load (browser mode)", () => {
  it("hydrates identity/settings/devices from the demo fixtures", async () => {
    await useData.getState().load();
    const s = useData.getState();
    expect(s.identity?.deviceId.startsWith("vJx0")).toBe(true);
    expect(s.settings?.discoverable).toBe(true);
    expect(s.devices.length).toBe(5);
    expect(s.devices.some((d) => d.deviceId === "demo-mini")).toBe(true);
    expect(s.downloadDir).toBe("~/Downloads/LanBeam");
    // port 0 sentinel resolves to the default listen port
    expect(s.listenPort).toBe(51704);
    expect(s.networkInfo[0]?.ip).toBe("192.168.1.20");
    // every demo device gets a firstSeen stamp
    expect(Object.keys(s.firstSeen).length).toBe(5);
  });

  it("does NOT overwrite prefs mirrors in browser mode", async () => {
    usePrefs.getState().set({ logLevel: "verbose" });
    await useData.getState().load();
    expect(usePrefs.getState().logLevel).toBe("verbose");
  });
});

describe("useData.refreshNetworkInfo (browser mode)", () => {
  it("adopts the demo network list", async () => {
    await useData.getState().refreshNetworkInfo();
    expect(useData.getState().networkInfo[0]?.ip).toBe("192.168.1.20");
  });
});

// ── useTransfers ─────────────────────────────────────────────────────────────
describe("useTransfers.upsert", () => {
  it("creates a transfer with sensible defaults", () => {
    useTransfers.getState().upsert({ sessionId: "s1", direction: "send" });
    const t = useTransfers.getState().transfers.s1;
    expect(t).toMatchObject({
      sessionId: "s1",
      direction: "send",
      status: "active",
      percent: 0,
    });
  });

  it("merges into an existing transfer", () => {
    useTransfers.getState().upsert({ sessionId: "s1", direction: "send" });
    useTransfers.getState().upsert({ sessionId: "s1", percent: 42 });
    const t = useTransfers.getState().transfers.s1;
    expect(t.percent).toBe(42);
    expect(t.direction).toBe("send"); // preserved
  });

  it("stamps doneAt and zeroes speed on a terminal transition", () => {
    vi.useFakeTimers();
    vi.setSystemTime(9000);
    useTransfers.getState().upsert({ sessionId: "s1", direction: "send" });
    useTransfers.getState().upsert({ sessionId: "s1", status: "done" });
    const t = useTransfers.getState().transfers.s1;
    expect(t.doneAt).toBe(9000);
    expect(t.speedBps).toBe(0);
  });

  it("attaches pending receive meta when the receive transfer appears", () => {
    const r: IncomingRequest = {
      sessionId: "rx",
      deviceId: "peer1",
      sas: "111222",
      totalSize: 100,
      fileCount: 1,
      files: [{ name: "doc.pdf", size: 100 }],
    };
    useTransfers.getState().acceptMeta(r, "Peer One");
    useTransfers.getState().upsert({ sessionId: "rx", direction: "receive" });
    const t = useTransfers.getState().transfers.rx;
    expect(t.peerId).toBe("peer1");
    expect(t.peerName).toBe("Peer One");
    expect(t.name).toBe("doc.pdf");
    expect(t.ext).toBe("PDF");
    expect(t.files).toEqual([{ name: "doc.pdf", size: 100 }]);
  });
});

describe("useTransfers.addTextTransfer", () => {
  it("inserts a terminal, file-less quick-text record", () => {
    useTransfers.getState().addTextTransfer({
      direction: "send",
      peerId: "p1",
      peerName: "Peer",
      text: "hello\n\nworld",
    });
    const rec = Object.values(useTransfers.getState().transfers)[0];
    expect(rec.kind).toBe("text");
    expect(rec.status).toBe("done");
    expect(rec.percent).toBe(100);
    expect(rec.ext).toBe("TXT");
    expect(rec.sessionId.startsWith("text-")).toBe(true);
    // whitespace collapsed for the preview
    expect(rec.name).toBe("hello world");
  });

  it("truncates a long preview to 48 chars + ellipsis", () => {
    useTransfers.getState().addTextTransfer({
      direction: "receive",
      peerId: "p1",
      peerName: "Peer",
      text: "x".repeat(80),
    });
    const rec = Object.values(useTransfers.getState().transfers)[0];
    expect(rec.name?.endsWith("…")).toBe(true);
    expect(rec.name).toBe(`${"x".repeat(48)}…`);
  });
});

describe("useTransfers.progress", () => {
  it("is a no-op for an unknown session", () => {
    const before = useTransfers.getState().transfers;
    useTransfers.getState().progress("nope", 50);
    expect(useTransfers.getState().transfers).toBe(before);
  });

  it("updates percent and promotes queued → active", () => {
    useTransfers
      .getState()
      .upsert({ sessionId: "p", direction: "receive", status: "queued" });
    useTransfers.getState().progress("p", 25, 1000);
    const t = useTransfers.getState().transfers.p;
    expect(t.percent).toBe(25);
    expect(t.status).toBe("active");
    expect(t.totalSize).toBe(1000);
  });

  it("never resurrects a terminal row (the recent guard fix)", () => {
    useTransfers
      .getState()
      .upsert({ sessionId: "p", direction: "send", status: "done" });
    useTransfers.getState().progress("p", 80);
    const t = useTransfers.getState().transfers.p;
    expect(t.status).toBe("done");
    expect(t.percent).toBe(0); // untouched
  });

  it("computes speed + a hist sample from consecutive ticks", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    useTransfers.getState().upsert({ sessionId: "p", direction: "receive" });
    // first tick: no prior sample, speed stays 0
    useTransfers.getState().progress("p", 10, 104857600); // 100 MB
    expect(useTransfers.getState().transfers.p.speedBps).toBe(0);
    expect(useTransfers.getState().transfers.p.hist).toEqual([]);
    // second tick 1s later: 10 % of 100 MB in 1 s = 10 MB/s
    vi.setSystemTime(1_001_000);
    useTransfers.getState().progress("p", 20);
    const t = useTransfers.getState().transfers.p;
    expect(t.speedBps).toBeCloseTo(10485760, 0);
    expect(t.hist).toEqual([10]);
    expect(t.percent).toBe(20);
  });
});

describe("useTransfers.setPaused", () => {
  it("flips the flag on a known session and no-ops otherwise", () => {
    useTransfers.getState().upsert({ sessionId: "p", direction: "send" });
    useTransfers.getState().setPaused("p", true);
    expect(useTransfers.getState().transfers.p.paused).toBe(true);
    const before = useTransfers.getState().transfers;
    useTransfers.getState().setPaused("ghost", true);
    expect(useTransfers.getState().transfers).toBe(before);
  });
});

describe("useTransfers per-file progress", () => {
  it("records a per-file tick", () => {
    useTransfers.getState().upsert({ sessionId: "p", direction: "receive" });
    useTransfers.getState().fileProgress("p", 0, 40);
    expect(useTransfers.getState().transfers.p.fileStat?.[0]).toEqual({
      percent: 40,
      done: false,
      verified: false,
    });
  });

  it("marks a file done + verified", () => {
    useTransfers.getState().upsert({ sessionId: "p", direction: "receive" });
    useTransfers.getState().fileDone("p", 0, true);
    expect(useTransfers.getState().transfers.p.fileStat?.[0]).toEqual({
      percent: 100,
      done: true,
      verified: true,
    });
  });

  it("won't un-finish a file already marked done", () => {
    useTransfers.getState().upsert({ sessionId: "p", direction: "receive" });
    useTransfers.getState().fileDone("p", 0, true);
    const before = useTransfers.getState().transfers;
    useTransfers.getState().fileProgress("p", 0, 55);
    expect(useTransfers.getState().transfers).toBe(before);
  });
});

describe("useTransfers.registerOutgoing + attachSas (pendingSend keying)", () => {
  const meta = (over: Partial<OutgoingMeta>): OutgoingMeta => ({
    peerId: "dev1",
    peerName: "Dev One",
    name: "f.txt",
    ext: "TXT",
    files: [{ name: "f.txt" }],
    paths: ["/f.txt"],
    ...over,
  });

  it("consumes queued metas FIFO, one per sas_code, and drops the empty key", () => {
    const t = useTransfers.getState();
    t.registerOutgoing("dev1", meta({ name: "f1.txt", paths: ["/f1.txt"] }));
    t.registerOutgoing("dev1", meta({ name: "f2.txt", paths: ["/f2.txt"] }));
    expect(useTransfers.getState().pendingSend.dev1).toHaveLength(2);

    t.attachSas("s1", "111", "dev1");
    expect(useTransfers.getState().transfers.s1.name).toBe("f1.txt");
    expect(useTransfers.getState().transfers.s1.sas).toBe("111");
    expect(useTransfers.getState().transfers.s1.peerId).toBe("dev1");
    expect(useTransfers.getState().pendingSend.dev1).toHaveLength(1);

    t.attachSas("s2", "222", "dev1");
    expect(useTransfers.getState().transfers.s2.name).toBe("f2.txt");
    // queue drained → key removed
    expect(useTransfers.getState().pendingSend.dev1).toBeUndefined();
  });

  it("still attaches a sas with no matching pending meta", () => {
    useTransfers.getState().attachSas("s3", "333", "unknownDev");
    const t = useTransfers.getState().transfers.s3;
    expect(t.sas).toBe("333");
    expect(t.peerId).toBe("unknownDev");
    expect(t.name).toBeUndefined();
  });
});

describe("useTransfers incoming queue", () => {
  const req = (id: string): IncomingRequest => ({
    sessionId: id,
    deviceId: "peer",
    sas: "000",
    totalSize: 0,
    fileCount: 0,
    files: [],
  });

  it("push / shift maintain FIFO order", () => {
    useTransfers.getState().pushIncoming(req("r1"));
    useTransfers.getState().pushIncoming(req("r2"));
    expect(useTransfers.getState().incomings.map((r) => r.sessionId)).toEqual([
      "r1",
      "r2",
    ]);
    useTransfers.getState().shiftIncoming();
    expect(useTransfers.getState().incomings.map((r) => r.sessionId)).toEqual([
      "r2",
    ]);
  });

  it("removeIncoming drops a specific card and no-ops when absent", () => {
    useTransfers.getState().pushIncoming(req("r1"));
    useTransfers.getState().pushIncoming(req("r2"));
    useTransfers.getState().removeIncoming("r1");
    expect(useTransfers.getState().incomings.map((r) => r.sessionId)).toEqual([
      "r2",
    ]);
    const before = useTransfers.getState().incomings;
    useTransfers.getState().removeIncoming("does-not-exist");
    expect(useTransfers.getState().incomings).toBe(before);
  });
});

describe("useTransfers.acceptMeta", () => {
  it("stores receive meta keyed by sessionId", () => {
    const r: IncomingRequest = {
      sessionId: "rx",
      deviceId: "peerX",
      sas: "999",
      totalSize: 10,
      fileCount: 2,
      files: [
        { name: "a.jpg", size: 4 },
        { name: "b.png", size: 6 },
      ],
    };
    useTransfers.getState().acceptMeta(r, "Peer X");
    expect(useTransfers.getState().pendingRecv.rx).toEqual({
      peerId: "peerX",
      peerName: "Peer X",
      name: "a.jpg",
      ext: "JPG",
      files: [
        { name: "a.jpg", size: 4 },
        { name: "b.png", size: 6 },
      ],
    });
  });
});

describe("useTransfers.removeTransfer", () => {
  it("deletes a transfer", () => {
    useTransfers.getState().upsert({ sessionId: "s1", direction: "send" });
    useTransfers.getState().removeTransfer("s1");
    expect(useTransfers.getState().transfers.s1).toBeUndefined();
  });
});

describe("useTransfers persistence (partialize — terminal rows only)", () => {
  it("persists only done/error rows, never active ones", () => {
    useTransfers
      .getState()
      .upsert({ sessionId: "act", direction: "send", status: "active" });
    useTransfers
      .getState()
      .upsert({ sessionId: "fin", direction: "send", status: "done" });
    const raw = localStorage.getItem("lanbeam.transfers");
    expect(raw).not.toBeNull();
    const keys = Object.keys(JSON.parse(raw as string).state.transfers);
    expect(keys).toContain("fin");
    expect(keys).not.toContain("act");
  });
});

// ── useTrust ─────────────────────────────────────────────────────────────────
describe("useTrust.setTrust", () => {
  it("creates a trusted record with autoAccept defaulted ON", () => {
    vi.useFakeTimers();
    vi.setSystemTime(2000);
    // Trusting a device also enables auto-accept — "these are my devices,
    // stop nagging me" (the drag-into-circle promise).
    useTrust.getState().setTrust({ deviceId: "d1", name: "Alice" }, true);
    const r = useTrust.getState().records.d1;
    expect(r).toMatchObject({
      deviceId: "d1",
      name: "Alice",
      trusted: true,
      autoAccept: true,
      addedAt: 2000,
    });
  });

  it("re-trusting an already-trusted device preserves an explicit auto-off", () => {
    const trust = useTrust.getState();
    trust.setTrust({ deviceId: "d1", name: "A" }, true); // auto defaults on
    trust.toggleAuto("d1"); // user turns it OFF for this device
    expect(useTrust.getState().records.d1.autoAccept).toBe(false);
    // Re-affirming trust (e.g. a re-drag) must NOT clobber that choice back on.
    useTrust.getState().setTrust({ deviceId: "d1", name: "A" }, true);
    expect(useTrust.getState().records.d1.autoAccept).toBe(false);
  });

  it("untrusting keeps the memo but clears trusted + autoAccept", () => {
    useTrust.getState().setTrust({ deviceId: "d1", name: "Alice" }, true);
    expect(useTrust.getState().records.d1.autoAccept).toBe(true); // fresh trust → on
    useTrust.getState().setTrust({ deviceId: "d1", name: "Alice" }, false);
    const r = useTrust.getState().records.d1;
    expect(r.trusted).toBe(false);
    expect(r.autoAccept).toBe(false);
    expect(r.name).toBe("Alice"); // memo preserved
  });
});

describe("useTrust.toggleAuto", () => {
  it("flips autoAccept on a known record and no-ops otherwise", () => {
    useTrust.getState().setTrust({ deviceId: "d1", name: "A" }, true);
    expect(useTrust.getState().records.d1.autoAccept).toBe(true); // trusting → on
    useTrust.getState().toggleAuto("d1");
    expect(useTrust.getState().records.d1.autoAccept).toBe(false);
    useTrust.getState().toggleAuto("d1");
    expect(useTrust.getState().records.d1.autoAccept).toBe(true);
    const before = useTrust.getState().records;
    useTrust.getState().toggleAuto("ghost");
    expect(useTrust.getState().records).toBe(before);
  });
});

describe("useTrust.touch", () => {
  it("refreshes lastSeen + name on a known record and no-ops otherwise", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1000);
    useTrust.getState().setTrust({ deviceId: "d1", name: "Old" }, true);
    vi.setSystemTime(5000);
    useTrust.getState().touch("d1", "New");
    const r = useTrust.getState().records.d1;
    expect(r.name).toBe("New");
    expect(r.lastSeen).toBe(5000);
    const before = useTrust.getState().records;
    useTrust.getState().touch("ghost", "X");
    expect(useTrust.getState().records).toBe(before);
  });
});

describe("useTrust.remove", () => {
  it("deletes a record and clears the selection if it pointed at it", () => {
    useTrust.getState().setTrust({ deviceId: "d1", name: "A" }, true);
    useTrust.getState().setSel("d1");
    useTrust.getState().remove("d1");
    expect(useTrust.getState().records.d1).toBeUndefined();
    expect(useTrust.getState().sel).toBeNull();
  });
});

describe("useTrust.hydrate", () => {
  it("adopts the backend list, converts stamps, and keeps untrusted memos", () => {
    // an untrusted local memo (rename) — has no backend row
    useTrust.getState().rename("memo1", "Just A Name");
    // a stale trusted record that the backend no longer knows
    useTrust.getState().setTrust({ deviceId: "stale", name: "Stale" }, true);

    const list: TrustedPeer[] = [
      {
        deviceId: "p1",
        name: "P1",
        autoAccept: true,
        pairedAt: 1000,
        lastSeen: 2000,
      },
    ];
    useTrust.getState().hydrate(list);
    const recs = useTrust.getState().records;

    expect(recs.memo1).toBeDefined(); // untrusted memo survives
    expect(recs.memo1.trusted).toBe(false);
    expect(recs.stale).toBeUndefined(); // stale trusted record dropped
    expect(recs.p1).toMatchObject({
      trusted: true,
      autoAccept: true,
      addedAt: 1_000_000, // unix s → epoch ms
      lastSeen: 2_000_000,
    });
  });
});

// ── useInbox ─────────────────────────────────────────────────────────────────
describe("useInbox", () => {
  const item = (id: string) => ({
    id,
    kind: "img" as const,
    ext: "JPG",
    name: id,
    from: "Peer",
    ts: 1,
    sizeBytes: 1,
    count: 1,
  });

  it("adds items newest-first and bumps unread", () => {
    useInbox.getState().add(item("a"));
    useInbox.getState().add(item("b"));
    expect(useInbox.getState().items.map((i) => i.id)).toEqual(["b", "a"]);
    expect(useInbox.getState().unread).toBe(2);
  });

  it("dedupes by id without inflating unread", () => {
    useInbox.getState().add(item("a"));
    useInbox.getState().add(item("a"));
    expect(useInbox.getState().items).toHaveLength(1);
    expect(useInbox.getState().unread).toBe(1);
  });

  it("removes by id list and clears unread", () => {
    useInbox.getState().add(item("a"));
    useInbox.getState().add(item("b"));
    useInbox.getState().remove(["a"]);
    expect(useInbox.getState().items.map((i) => i.id)).toEqual(["b"]);
    useInbox.getState().clearUnread();
    expect(useInbox.getState().unread).toBe(0);
  });
});

describe("inboxFromTransfer", () => {
  it("builds an inbox item from a completed inbound transfer", () => {
    const t = {
      sessionId: "sx",
      savedNames: ["holiday.jpg"],
      fileCount: 3,
      totalSize: 4096,
      peerName: "Bob",
    } as UITransfer;
    const item = inboxFromTransfer(t, ["/dl/holiday.jpg"]);
    expect(item).toMatchObject({
      id: "sx",
      kind: "img",
      ext: "JPG",
      name: "holiday.jpg",
      from: "Bob",
      sizeBytes: 4096,
      count: 3,
      sessionId: "sx",
      paths: ["/dl/holiday.jpg"],
    });
  });

  it("falls back through files / name when savedNames is absent", () => {
    const t = {
      sessionId: "sy",
      files: [{ name: "notes.pdf" }],
      totalSize: 10,
    } as UITransfer;
    const item = inboxFromTransfer(t, []);
    expect(item.name).toBe("notes.pdf");
    expect(item.ext).toBe("PDF");
    expect(item.kind).toBe("doc");
    expect(item.count).toBe(1);
  });
});

describe("inboxFromText", () => {
  it("builds a txt inbox item using the backend receive stamp", () => {
    const item = inboxFromText("Alice", "  hi  there  ", 12345);
    expect(item.kind).toBe("txt");
    expect(item.ext).toBe("TXT");
    expect(item.from).toBe("Alice");
    expect(item.ts).toBe(12345);
    expect(item.sizeBytes).toBe(0);
    expect(item.count).toBe(1);
    expect(item.name).toBe("hi there"); // collapsed
    expect(item.text).toBe("  hi  there  "); // raw preserved
    expect(item.id.startsWith("txt-")).toBe(true);
  });

  it("truncates a long preview", () => {
    const item = inboxFromText("Alice", "y".repeat(80), 1);
    expect(item.name).toBe(`${"y".repeat(48)}…`);
  });
});

// ── useRecents ───────────────────────────────────────────────────────────────
describe("useRecents", () => {
  const f = (name: string) => ({ name, ext: "TXT" });

  it("prepends new files", () => {
    useRecents.getState().add([f("a"), f("b")]);
    expect(useRecents.getState().items.map((i) => i.name)).toEqual(["a", "b"]);
  });

  it("dedupes and moves a re-added file to the front", () => {
    useRecents.getState().add([f("a"), f("b")]);
    useRecents.getState().add([f("b")]);
    expect(useRecents.getState().items.map((i) => i.name)).toEqual(["b", "a"]);
  });

  it("caps the list at 8", () => {
    useRecents.getState().add(Array.from({ length: 12 }, (_, i) => f(`f${i}`)));
    expect(useRecents.getState().items).toHaveLength(8);
  });
});

// ── useToast ─────────────────────────────────────────────────────────────────
describe("useToast", () => {
  beforeEach(() => vi.useFakeTimers());

  it("shows a message and auto-hides after the default timeout", () => {
    useToast.getState().show("saved");
    expect(useToast.getState().msg).toBe("saved");
    expect(useToast.getState().action).toBeNull();
    vi.advanceTimersByTime(3200);
    expect(useToast.getState().msg).toBeNull();
  });

  it("keeps an action and honors a custom duration", () => {
    const fn = vi.fn();
    useToast.getState().show("undo?", { label: "Undo", fn }, 1000);
    expect(useToast.getState().action?.label).toBe("Undo");
    vi.advanceTimersByTime(999);
    expect(useToast.getState().msg).toBe("undo?");
    vi.advanceTimersByTime(1);
    expect(useToast.getState().msg).toBeNull();
  });

  it("hide clears immediately", () => {
    useToast.getState().show("hi");
    useToast.getState().hide();
    expect(useToast.getState().msg).toBeNull();
  });
});

// ── useOverlays ──────────────────────────────────────────────────────────────
describe("useOverlays.setPair / pairPrefill", () => {
  it("stashes a prefill on open and clears it on close", () => {
    useOverlays.getState().setPair(true, "lanbeam://pair?x=1");
    expect(useOverlays.getState().pairOpen).toBe(true);
    expect(useOverlays.getState().pairPrefill).toBe("lanbeam://pair?x=1");
    useOverlays.getState().setPair(false);
    expect(useOverlays.getState().pairOpen).toBe(false);
    expect(useOverlays.getState().pairPrefill).toBeNull();
  });

  it("opening without a prefill leaves it null", () => {
    useOverlays.getState().setPair(true);
    expect(useOverlays.getState().pairPrefill).toBeNull();
    useOverlays.getState().setPairPrefill("later");
    expect(useOverlays.getState().pairPrefill).toBe("later");
  });
});

describe("useOverlays.openSend", () => {
  it("presets the device and seeds an empty selection", () => {
    useOverlays.getState().openSend("d1", [{ name: "a.txt", ext: "TXT" }]);
    const send = useOverlays.getState().send;
    expect(send?.step).toBe("files");
    expect(send?.preset).toBe(true);
    expect(send?.deviceIds).toEqual(["d1"]);
    expect(send?.sel).toEqual([]);
    expect(send?.pool).toHaveLength(1);
  });

  it("merges presel ahead of the pool, deduped, and preselects it", () => {
    useOverlays.getState().openSend(
      null,
      [
        { name: "dup.txt", ext: "TXT" },
        { name: "other.txt", ext: "TXT" },
      ],
      [{ name: "dup.txt", ext: "TXT" }],
    );
    const send = useOverlays.getState().send;
    expect(send?.preset).toBe(false);
    expect(send?.deviceIds).toEqual([]);
    expect(send?.pool.map((f) => f.name)).toEqual(["dup.txt", "other.txt"]);
    expect(send?.sel).toEqual(["dup.txt"]);
  });
});

describe("useOverlays.patchSend / closeSend", () => {
  it("patchSend no-ops when no send flow is open", () => {
    useOverlays.getState().patchSend({ step: "device" });
    expect(useOverlays.getState().send).toBeNull();
  });

  it("patchSend merges into an open flow, closeSend clears it", () => {
    useOverlays.getState().openSend("d1", []);
    useOverlays.getState().patchSend({ step: "confirm" });
    expect(useOverlays.getState().send?.step).toBe("confirm");
    useOverlays.getState().closeSend();
    expect(useOverlays.getState().send).toBeNull();
  });
});

describe("useOverlays.setConflict + misc setters", () => {
  it("sets and clears the pending conflict", () => {
    const conflict = {
      request: {
        sessionId: "c1",
        deviceId: "peer",
        sas: "1",
        totalSize: 0,
        fileCount: 0,
        files: [],
      },
      peerName: "Peer",
      wantTrust: true,
    };
    useOverlays.getState().setConflict(conflict);
    expect(useOverlays.getState().conflict).toBe(conflict);
    useOverlays.getState().setConflict(null);
    expect(useOverlays.getState().conflict).toBeNull();
  });

  it("toggles the drag state with an optional device", () => {
    useOverlays.getState().setDrag(true, "d1");
    expect(useOverlays.getState().dragOver).toBe(true);
    expect(useOverlays.getState().dragDevice).toBe("d1");
    useOverlays.getState().setDrag(false);
    expect(useOverlays.getState().dragOver).toBe(false);
    expect(useOverlays.getState().dragDevice).toBeNull();
  });
});

describe("历史记录保留 (histKeep)", () => {
  const DAY = 24 * 3600_000;

  beforeEach(() => {
    useTransfers.setState({ transfers: {} });
    usePrefs.getState().set({ histKeep: "30d" });
  });

  it("maps each option to a window — and 「不保留」 really is zero", () => {
    expect(histWindowMs("none")).toBe(0);
    expect(histWindowMs("7d")).toBe(7 * DAY);
    expect(histWindowMs("30d")).toBe(30 * DAY);
    expect(histWindowMs("forever")).toBe(Number.POSITIVE_INFINITY);
  });

  it("drops rows that have outlived the window, keeps the ones inside it", () => {
    const now = Date.now();
    useTransfers.setState({
      transfers: {
        old: {
          sessionId: "old",
          status: "done",
          doneAt: now - 40 * DAY,
        } as never,
        recent: {
          sessionId: "recent",
          status: "done",
          doneAt: now - 2 * DAY,
        } as never,
      },
    });
    usePrefs.getState().set({ histKeep: "30d" });
    useTransfers.getState().pruneHistory();

    const left = useTransfers.getState().transfers;
    expect(left.old).toBeUndefined();
    expect(left.recent).toBeDefined();
  });

  it("never prunes a transfer that is still running", () => {
    // An in-flight row has no doneAt. Reading that as "epoch 0, ancient" and
    // deleting it would drop the transfer the user is watching right now.
    useTransfers.setState({
      transfers: {
        live: { sessionId: "live", status: "active" } as never,
      },
    });
    usePrefs.getState().set({ histKeep: "7d" });
    useTransfers.getState().pruneHistory();
    expect(useTransfers.getState().transfers.live).toBeDefined();
  });

  it("「不保留」 stops persistence without wiping the list you are looking at", () => {
    // "Don't KEEP them" is not "delete the ones on screen". A transfer that just
    // finished must stay visible for the rest of the session.
    useTransfers.setState({
      transfers: {
        justNow: {
          sessionId: "justNow",
          status: "done",
          doneAt: Date.now(),
        } as never,
      },
    });
    usePrefs.getState().set({ histKeep: "none" });
    useTransfers.getState().pruneHistory();
    expect(useTransfers.getState().transfers.justNow).toBeDefined();
  });

  it("「永久保留」 keeps even an ancient row", () => {
    useTransfers.setState({
      transfers: {
        ancient: {
          sessionId: "ancient",
          status: "done",
          doneAt: 1,
        } as never,
      },
    });
    usePrefs.getState().set({ histKeep: "forever" });
    useTransfers.getState().pruneHistory();
    expect(useTransfers.getState().transfers.ancient).toBeDefined();
  });
});

describe("忘记设备 ≠ 让设备消失", () => {
  const dev = (deviceId: string, name: string): DiscoveredDevice =>
    ({ deviceId, name, address: "192.168.1.9" }) as DiscoveredDevice;

  it("a forgotten device that is STILL on the LAN stays listed — as a stranger", () => {
    // The trust page is a map of your network, not a list of your trust records.
    // Dropping the record does not un-announce the device: discovery keeps seeing
    // it, so it comes back untrusted and outside the ring. This is the answer to
    // "I removed it, why is it still there" — and the copy now says so.
    const list = trustList([dev("nas", "NAS")], {});
    expect(list).toHaveLength(1);
    expect(list[0]).toMatchObject({
      deviceId: "nas",
      trusted: false,
      online: true,
    });
  });

  it("a forgotten device that is NOT on the LAN really does disappear", () => {
    // With no record and no announcement, there is nothing left to draw.
    expect(trustList([], {})).toHaveLength(0);
  });

  it("un-trusting keeps the device (and its memo); it is not a removal", () => {
    const records: Record<string, TrustRecord> = {
      nas: {
        deviceId: "nas",
        name: "我的 NAS",
        trusted: false,
        autoAccept: false,
        addedAt: 1,
        lastSeen: 1,
      },
    };
    const list = trustList([dev("nas", "NAS")], records);
    expect(list).toHaveLength(1);
    // The custom name survives — that is exactly what "forget" would clear.
    expect(list[0]).toMatchObject({ name: "我的 NAS", trusted: false });
  });
});

describe("指纹变化告警:名字必须真的说明问题", () => {
  const dev = (deviceId: string, name: string): DiscoveredDevice =>
    ({ deviceId, name, address: "1.1.1.1", port: 1 }) as DiscoveredDevice;
  const rec = (deviceId: string, name: string): TrustRecord => ({
    deviceId,
    name,
    trusted: true,
    autoAccept: true,
    addedAt: 1,
    lastSeen: 1,
  });

  it("does NOT cry wolf at two devices that merely share a name", () => {
    // The default device name is the machine's hostname, falling back to
    // "LanBeam device" — so every install nobody renamed answers to the same
    // thing. Flagging 「指纹已变化」 on that is an impersonation alarm raised
    // because two people didn't rename their laptops.
    const out = trustList([dev("live", "LanBeam device")], {
      old: rec("old", "LanBeam device"),
      live: rec("live", "LanBeam device"), // already known in its own right
    });
    const offline = out.find((d) => !d.online);
    expect(offline?.fpChanged).toBeUndefined();
  });

  it("does NOT flag a live device you already know in its own right", () => {
    // A device you have a record for is not impersonating anybody.
    const out = trustList([dev("b", "Studio")], {
      a: rec("a", "Studio"),
      b: rec("b", "Studio"),
    });
    expect(out.find((d) => d.deviceId === "a")?.fpChanged).toBeUndefined();
  });

  it("does NOT flag when several live devices wear the name", () => {
    // If three machines answer to it, the name is evidence of nothing.
    const out = trustList([dev("x", "Shared"), dev("y", "Shared")], {
      old: rec("old", "Shared"),
    });
    expect(out.find((d) => d.deviceId === "old")?.fpChanged).toBeUndefined();
  });

  it("DOES flag the one case it is for: a single unknown device wearing a remembered name", () => {
    const out = trustList([dev("newkey", "书房 · MacBook")], {
      oldkey: rec("oldkey", "书房 · MacBook"),
    });
    expect(out.find((d) => d.deviceId === "oldkey")?.fpChanged).toEqual({
      newDeviceId: "newkey",
    });
  });
});
