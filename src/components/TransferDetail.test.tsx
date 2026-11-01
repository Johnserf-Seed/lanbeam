// The detail drawer reports verification from what the transfer's own file
// events said (M13) — not from this machine's 完整性校验 setting, which decides
// what THIS machine sends, and says nothing about a receive.
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import type { UITransfer } from "../lib/store";
import { useOverlays, usePrefs, useTransfers } from "../lib/store";
import { renderUI, screen } from "../test/render";
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
