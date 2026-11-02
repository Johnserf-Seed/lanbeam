// The detail drawer reports verification from what the transfer's own file
// events said (M13) — not from this machine's 完整性校验 setting, which decides
// what THIS machine sends, and says nothing about a receive.
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import { fmtBytes } from "../lib/format";
import type { UITransfer } from "../lib/store";
import { useOverlays, usePrefs, useTransfers } from "../lib/store";
import { fireEvent, renderUI, screen } from "../test/render";
import TransferDetail from "./TransferDetail";

const received = (over: Partial<UITransfer> = {}): UITransfer => ({
  sessionId: "r1",
  direction: "receive",
  status: "done",
  totalSize: 20,
  percent: 100,
  speedBps: 0,
  hist: [],
  startedAt: 1,
  doneAt: 2,
  name: "a.bin",
  peerName: "Studio",
  files: [
    { name: "a.bin", size: 10 },
    { name: "b.bin", size: 10 },
  ],
  ...over,
});

const show = (tr: UITransfer) => {
  useTransfers.setState({ transfers: { [tr.sessionId]: tr } });
  useOverlays.setState({ detailId: tr.sessionId });
  renderUI(<TransferDetail />);
};

beforeEach(() => {
  useOverlays.setState({ detailId: null });
});

describe("TransferDetail · verification (M13)", () => {
  it("doesn't claim a check nobody made, whatever this machine's setting", () => {
    usePrefs.setState({ verifyHash: true });
    show(received());
    expect(screen.getByText(i18n.t("detail.footNotVerified"))).toBeTruthy();
    expect(screen.queryByText(i18n.t("detail.evVerified"))).toBeNull();
    expect(screen.queryByText(i18n.t("detail.fileVerified"))).toBeNull();
  });

  it("counts the files that were verified, even with the setting off here", () => {
    usePrefs.setState({ verifyHash: false });
    show(
      received({
        fileStat: {
          0: { percent: 100, done: true, verified: true },
          1: { percent: 100, done: true, verified: false },
        },
      }),
    );
    expect(
      screen.getByText(i18n.t("detail.footVerified", { n: 1, total: 2 })),
    ).toBeTruthy();
    expect(
      screen.getByText(i18n.t("detail.evVerifiedSub", { n: 1, total: 2 })),
    ).toBeTruthy();
  });
});

describe("TransferDetail · by folder (F5)", () => {
  const named = (...names: string[]) =>
    names.map((name) => ({ name, size: 10 }));
  const folder = (dir: string) =>
    screen.getByText(dir).closest("button") as HTMLButtonElement;

  it("lists a folder's files under it, each by its own name", () => {
    show(
      received({
        totalSize: 30,
        fileCount: 3,
        files: named("a.txt", "Trip/map.png", "Trip/day1/photo.jpg"),
      }),
    );
    expect(folder("Trip").getAttribute("aria-expanded")).toBe("true");
    expect(folder("Trip/day1").getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("a.txt")).toBeTruthy();
    expect(screen.getByText("map.png")).toBeTruthy();
    expect(screen.getByText("photo.jpg")).toBeTruthy();
    expect(screen.queryByText("Trip/map.png")).toBeNull();
    // Done, so every folder counts all of its files as done.
    expect(folder("Trip").textContent).toContain("1/1");
  });

  it("a send of loose files stays one plain list", () => {
    show(received({ fileCount: 2, files: named("a.bin", "b.bin") }));
    expect(document.querySelectorAll("button[aria-expanded]")).toHaveLength(0);
    expect(screen.getByText("b.bin")).toBeTruthy();
  });

  it("a big transfer opens with its folders closed, and a click opens one", () => {
    const many = Array.from({ length: 60 }, (_, i) => `Big/f${i}.bin`);
    show(received({ totalSize: 600, fileCount: 60, files: named(...many) }));
    const big = folder("Big");
    expect(big.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("f0.bin")).toBeNull();
    fireEvent.click(big);
    expect(folder("Big").getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("f0.bin")).toBeTruthy();
  });

  it("a row restored from history says how many files it no longer lists", () => {
    show(
      received({ fileCount: 150, files: named("Big/f0.bin", "Big/f1.bin") }),
    );
    expect(
      screen.getByText(i18n.t("detail.moreFiles", { count: 148 })),
    ).toBeTruthy();
    // It counts all 150, not the two names still listed.
    expect(
      screen.getByText(
        i18n.t("detail.sizeLine", { count: 150, size: fmtBytes(20) }),
      ),
    ).toBeTruthy();
  });
});
