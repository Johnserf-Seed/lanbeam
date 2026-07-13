/** Browser-mode demo seeding (?demo=1): populates stores with the prototype's
 *  sample content so every screen can be design-reviewed without a backend.
 *  Never runs inside Tauri. */
import { isTauri } from "../bridge/api";
import { useInbox, useTransfers, useTrust, type UITransfer } from "./store";

let seeded = false;

export function maybeSeedDemo(): void {
  if (isTauri || seeded) return;
  if (!new URLSearchParams(window.location.search).has("demo")) return;
  seeded = true;

  const now = Date.now();
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
        name: "产品设计稿 v2.zip",
        ext: "ZIP",
        peerId: "demo-mini",
        peerName: "客厅 · Mac mini",
        totalSize: 1229 * 1048576,
        fileCount: 1,
        files: [{ name: "产品设计稿 v2.zip", size: 1229 * 1048576 }],
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
        peerName: "小敏的手机",
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
        name: "季度汇报.key",
        ext: "KEY",
        peerId: "demo-tp",
        peerName: "工位 · ThinkPad",
        totalSize: 86 * 1048576,
        fileCount: 1,
        files: [{ name: "季度汇报.key", size: 86 * 1048576 }],
        percent: 100,
        status: "done",
        startedAt: now - 86400_000,
        doneAt: now - 86300_000,
      }),
      d4: mk({
        sessionId: "d4",
        direction: "receive",
        name: "素材包.zip",
        ext: "ZIP",
        peerId: "demo-nas",
        peerName: "NAS · Synology",
        totalSize: 3481 * 1048576,
        fileCount: 1,
        files: [{ name: "素材包.zip", size: 3481 * 1048576 }],
        percent: 62,
        status: "error",
        error: "连接中断",
        startedAt: now - 90000_000,
        doneAt: now - 89950_000,
      }),
      // Quick-text history entries (M7.3): a sent + a received text record so the
      // unified「everything I sent/received」history is design-reviewable.
      t1: mk({
        sessionId: "text-demo-1",
        kind: "text",
        direction: "send",
        text: "会议室改到 3 楼 302，记得带 HDMI 线",
        name: "会议室改到 3 楼 302，记得带 HDMI 线",
        ext: "TXT",
        peerId: "demo-mini",
        peerName: "客厅 · Mac mini",
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
        text: "https://figma.com/file/lb-review\n评审改到周四 10:30，帮我转给组里",
        name: "https://figma.com/file/lb-review 评审改到周四…",
        ext: "TXT",
        peerId: "demo-min",
        peerName: "小敏的手机",
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
        name: "客厅 · Mac mini",
        trusted: true,
        autoAccept: true,
        addedAt: now - 86400_000,
        lastSeen: now,
        pos: { x: 250, y: 120 },
      },
      "demo-nas": {
        deviceId: "demo-nas",
        name: "NAS · Synology",
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
        name: "出差照片 ×24",
        from: "客厅 · Mac mini",
        ts: now - 2 * 3600_000,
        sizeBytes: 186 * 1048576,
        count: 24,
      },
      {
        id: "s2",
        kind: "doc",
        ext: "KEY",
        name: "季度汇报 (2).key",
        from: "工位 · ThinkPad",
        ts: now - 5 * 3600_000,
        sizeBytes: 24.6 * 1048576,
        count: 1,
      },
      {
        id: "s3",
        kind: "vid",
        ext: "MOV",
        name: "家庭录像_0705.mov",
        from: "NAS · Synology",
        ts: now - 30 * 3600_000,
        sizeBytes: 860 * 1048576,
        count: 1,
      },
      {
        id: "s4",
        kind: "txt",
        ext: "TXT",
        name: "文本 · figma.com/file/lb-review…",
        from: "客厅 · Mac mini",
        ts: now - 32 * 3600_000,
        sizeBytes: 0,
        count: 1,
        text: "https://figma.com/file/lb-review\n评审改到周四 10:30，帮我转给组里",
      },
      {
        id: "s5",
        kind: "img",
        ext: "PNG",
        name: "白板拍照.png",
        from: "iPad Air",
        ts: now - 5 * 86400_000,
        sizeBytes: 2.1 * 1048576,
        count: 1,
      },
    ],
    unread: 0,
  });
}
