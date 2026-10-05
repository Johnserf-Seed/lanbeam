/** Browser-mode demo seeding (?demo=1): populates stores with the prototype's
 *  sample content so every screen can be design-reviewed without a backend.
 *  The names and texts follow the UI language (see demoContent). Never runs
 *  inside Tauri. */
import { isTauri } from "../bridge/api";
import { demoContent } from "./demoContent";
import {
  inboxFromText,
  useInbox,
  useTransfers,
  useTrust,
  type UITransfer,
} from "./store";

let seeded = false;

export function maybeSeedDemo(): void {
  if (isTauri || seeded) return;
  if (!new URLSearchParams(window.location.search).has("demo")) return;
  seeded = true;

  const now = Date.now();
  const c = demoContent();
  const mk = (t: Partial<UITransfer> & { sessionId: string }): UITransfer => ({
    direction: "send",
    totalSize: 0,
    percent: 0,
    status: "active",
    speedBps: 0,
    hist: [],
    startedAt: now,
    ...t,
  });

  const hist1 = [
    38, 42, 45, 41, 50, 47, 52, 44, 48, 55, 49, 58, 53, 47, 51, 46, 54, 60, 57,
    50, 45, 52, 48, 49,
  ];
  useTransfers.setState((s) => ({
    transfers: {
      ...s.transfers,
      d1: mk({
        sessionId: "d1",
        direction: "send",
        name: c.designZip,
        ext: "ZIP",
        peerId: "demo-mini",
        peerName: c.peers.mini,
        totalSize: 1229 * 1048576,
        fileCount: 1,
        files: [{ name: c.designZip, size: 1229 * 1048576 }],
        percent: 38,
        speedBps: 48.6 * 1048576,
        hist: hist1,
        started: true,
        startedAt: now - 60_000,
      }),
      d2: mk({
        sessionId: "d2",
        direction: "receive",
        name: "IMG_0231.HEIC",
        ext: "HEIC",
        peerId: "demo-min",
        peerName: c.peers.phone,
        totalSize: 72 * 1048576,
        fileCount: 1,
        files: [{ name: "IMG_0231.HEIC", size: 72 * 1048576 }],
        percent: 100,
        status: "done",
        startedAt: now - 3600_000,
        doneAt: now - 3540_000,
        savedNames: ["IMG_0231.HEIC"],
      }),
      d3: mk({
        sessionId: "d3",
        direction: "send",
        name: c.reviewKey,
        ext: "KEY",
        peerId: "demo-tp",
        peerName: c.peers.thinkpad,
        totalSize: 86 * 1048576,
        fileCount: 1,
        files: [{ name: c.reviewKey, size: 86 * 1048576 }],
        percent: 100,
        status: "done",
        startedAt: now - 86400_000,
        doneAt: now - 86300_000,
      }),
      d4: mk({
        sessionId: "d4",
        direction: "receive",
        name: c.footageZip,
        ext: "ZIP",
        peerId: "demo-nas",
        peerName: c.peers.nas,
        totalSize: 3481 * 1048576,
        fileCount: 1,
        files: [{ name: c.footageZip, size: 3481 * 1048576 }],
        percent: 62,
        status: "error",
        error: c.connectionLost,
        startedAt: now - 90000_000,
        doneAt: now - 89950_000,
      }),
      // Quick-text history entries (M7.3): a sent + a received text record so the
      // unified「everything I sent/received」history is design-reviewable.
      t1: mk({
        sessionId: "text-demo-1",
        kind: "text",
        direction: "send",
        text: c.textSent,
        name: c.textSent,
        ext: "TXT",
        peerId: "demo-mini",
        peerName: c.peers.mini,
        fileCount: 1,
        percent: 100,
        status: "done",
        startedAt: now - 1800_000,
        doneAt: now - 1800_000,
      }),
      t2: mk({
        sessionId: "text-demo-2",
        kind: "text",
        direction: "receive",
        text: c.textReceived,
        name: inboxFromText("", c.textReceived, 0).name,
        ext: "TXT",
        peerId: "demo-min",
        peerName: c.peers.phone,
        fileCount: 1,
        percent: 100,
        status: "done",
        startedAt: now - 7200_000,
        doneAt: now - 7200_000,
      }),
    },
    incomings: [
      {
        sessionId: "demo-inc",
        deviceId: "demo-min",
        sas: "483921",
        totalSize: 214 * 1048576,
        fileCount: 3,
        files: [
          { name: "IMG_0231.HEIC", size: 70 * 1048576 },
          { name: "IMG_0245.HEIC", size: 72 * 1048576 },
          { name: "VID_0246.MOV", size: 72 * 1048576 },
        ],
      },
    ],
  }));

  useTrust.setState({
    records: {
      "demo-mini": {
        deviceId: "demo-mini",
        name: c.peers.mini,
        trusted: true,
        autoAccept: true,
        addedAt: now - 86400_000,
        lastSeen: now,
        pos: { x: 250, y: 120 },
      },
      "demo-nas": {
        deviceId: "demo-nas",
        name: c.peers.nas,
        trusted: true,
        autoAccept: false,
        addedAt: now - 86400_000,
        lastSeen: now,
        pos: { x: 380, y: 290 },
      },
    },
  });

  useInbox.setState({
    items: [
      {
        id: "s1",
        kind: "img",
        ext: "IMG",
        name: c.tripPhotos,
        from: c.peers.mini,
        ts: now - 2 * 3600_000,
        sizeBytes: 186 * 1048576,
        count: 24,
      },
      {
        id: "s2",
        kind: "doc",
        ext: "KEY",
        name: c.reviewKeyCopy,
        from: c.peers.thinkpad,
        ts: now - 5 * 3600_000,
        sizeBytes: 24.6 * 1048576,
        count: 1,
      },
      {
        id: "s3",
        kind: "vid",
        ext: "MOV",
        name: c.homeVideo,
        from: c.peers.nas,
        ts: now - 30 * 3600_000,
        sizeBytes: 860 * 1048576,
        count: 1,
      },
      // Built the way a real received text is, so the row matches the app.
      {
        ...inboxFromText(c.peers.mini, c.textReceived, now - 32 * 3600_000),
        id: "s4",
      },
      {
        id: "s5",
        kind: "img",
        ext: "PNG",
        name: c.whiteboard,
        from: c.peers.ipad,
        ts: now - 5 * 86400_000,
        sizeBytes: 2.1 * 1048576,
        count: 1,
      },
    ],
    unread: 0,
  });
}
