// Component tests for TransfersPage: the segmented all/active/done filter, the
// Running vs History split, a history row's name/size/direction, the Resend
// action on a failed send, opening the detail drawer by clicking a row, and the
// copy action on a quick-text history entry. The env is non-Tauri, so sendops is
// mocked to observe copyText / resendTransfer without hitting the bridge.
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import type { UITransfer } from "../lib/store";
import { useOverlays, useTransfers } from "../lib/store";
import { fireEvent, renderUI, screen } from "../test/render";
import TransfersPage from "./TransfersPage";

// The page calls copyText (text rows) and resendTransfer (failed sends) on
// interaction; mock both so we can assert the calls in browser mode.
vi.mock("../lib/sendops", () => ({
  copyText: vi.fn(),
  resendTransfer: vi.fn(),
}));
import { copyText, resendTransfer } from "../lib/sendops";

const MB = 1048576;

function tr(over: Partial<UITransfer> & { sessionId: string }): UITransfer {
  return {
    direction: "send",
    totalSize: 0,
    percent: 0,
    status: "active",
    speedBps: 0,
    hist: [],
    startedAt: 0,
    ...over,
  };
}

// Distinct rows across every category the page renders.
const active = tr({
  sessionId: "act",
  direction: "send",
  status: "active",
  name: "movie.mp4",
  peerName: "Alpha",
  totalSize: 10 * MB,
  percent: 30,
  startedAt: 400,
});
const done = tr({
  sessionId: "fin",
  direction: "receive",
  status: "done",
  name: "photo.png",
  peerName: "Bravo",
  totalSize: 5 * MB,
  percent: 100,
  startedAt: 300,
  doneAt: 1000,
});
const failed = tr({
  sessionId: "err",
  direction: "send",
  status: "error",
  name: "report.pdf",
  peerName: "Charlie",
  peerId: "dev-charlie",
  paths: ["/report.pdf"],
  totalSize: 2 * MB,
  percent: 40,
  startedAt: 200,
  doneAt: 900,
});
const text = tr({
  sessionId: "text-1",
  kind: "text",
  direction: "send",
  status: "done",
  name: "hello world",
  text: "hello world",
  peerName: "Delta",
  percent: 100,
  startedAt: 100,
  doneAt: 800,
});

beforeEach(() => {
  vi.clearAllMocks();
  // Merge-set only the slices we touch so the store's action fns survive.
  useTransfers.setState({
    transfers: {
      act: active,
      fin: done,
      err: failed,
      "text-1": text,
    },
  });
  useOverlays.setState({ detailId: null });
});

describe("TransfersPage", () => {
  it("renders without throwing and shows both cards with all rows", () => {
    renderUI(<TransfersPage />);
    // History header is unique; the Running card is evidenced by its active row
    // ("Active" alone is ambiguous with the filter button label).
    expect(screen.getByText(i18n.t("transfers.history"))).toBeTruthy();
    // One row per seeded transfer.
    expect(screen.getByText("movie.mp4")).toBeTruthy();
    expect(screen.getByText("photo.png")).toBeTruthy();
    expect(screen.getByText("report.pdf")).toBeTruthy();
    expect(screen.getByText("hello world")).toBeTruthy();
  });

  it("segmented filter hides History on 'active' and Running on 'done'", () => {
    renderUI(<TransfersPage />);

    // Default 'all': the active row (Running-only) and History header both show.
    expect(screen.getByText("movie.mp4")).toBeTruthy();
    expect(screen.getByText(i18n.t("transfers.history"))).toBeTruthy();

    // 'active' → History card gone, Running row stays.
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("transfers.filterActive") }),
    );
    expect(screen.queryByText(i18n.t("transfers.history"))).toBeNull();
    expect(screen.getByText("movie.mp4")).toBeTruthy();

    // 'done' → Running card gone, History rows stay.
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("transfers.filterDone") }),
    );
    expect(screen.queryByText("movie.mp4")).toBeNull();
    expect(screen.getByText(i18n.t("transfers.history"))).toBeTruthy();
    expect(screen.getByText("photo.png")).toBeTruthy();
  });

  it("renders a history row with name, size, and direction badge", () => {
    renderUI(<TransfersPage />);
    // name
    expect(screen.getByText("photo.png")).toBeTruthy();
    // size: fmtBytes(5 MB) → "5.0 MB"
    expect(screen.getByText("5.0 MB")).toBeTruthy();
    // direction: the received row shows the ↓ RECV pill (at least one exists)
    expect(
      screen.getAllByText(i18n.t("transfers.dirIn")).length,
    ).toBeGreaterThan(0);
  });

  it("the Resend action on a failed send calls resendTransfer", () => {
    renderUI(<TransfersPage />);
    const resendEl = screen.getByText(i18n.t("transfers.resend"));
    fireEvent.click(resendEl);
    expect(vi.mocked(resendTransfer)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(resendTransfer).mock.calls[0][0]).toMatchObject({
      sessionId: "err",
    });
    // stopPropagation: the row's own click (open detail) must not have fired.
    expect(useOverlays.getState().detailId).toBeNull();
  });

  it("clicking a history row opens the detail drawer (useOverlays.detailId)", () => {
    renderUI(<TransfersPage />);
    fireEvent.click(screen.getByText("photo.png"));
    expect(useOverlays.getState().detailId).toBe("fin");
  });

  it("clicking a text row copies its text", () => {
    renderUI(<TransfersPage />);
    fireEvent.click(screen.getByText("hello world"));
    expect(vi.mocked(copyText)).toHaveBeenCalledWith("hello world");
  });
});
