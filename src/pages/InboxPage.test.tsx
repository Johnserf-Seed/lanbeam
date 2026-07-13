// Component tests for InboxPage: day-grouping, category filter, row actions
// (open / reveal / copy) and the forward flow. The env is non-Tauri, so the
// real sendops helpers would only toast browser-mode stubs — we mock the whole
// module and assert the component calls the right op with the right args.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, renderUI, screen } from "../test/render";
import i18n from "../i18n";
import type { DiscoveredDevice } from "../bridge/api";
import {
  useData,
  useInbox,
  useOverlays,
  usePrefs,
  useSysDark,
  useToast,
  type InboxItem,
} from "../lib/store";
import InboxPage from "./InboxPage";

// The row/menu actions delegate to sendops; mock it so we can assert the calls
// without touching the clipboard / opener plugins. errText stays a pass-through
// so the forward-error path (which the component wraps in a toast) never throws.
vi.mock("../lib/sendops", () => ({
  copyText: vi.fn(),
  errText: (e: unknown) => String(e),
  openDir: vi.fn(() => Promise.resolve()),
  openFile: vi.fn(() => Promise.resolve()),
  revealFile: vi.fn(() => Promise.resolve()),
  sendTextTracked: vi.fn(() => Promise.resolve()),
  sendToDevice: vi.fn(() => true),
}));

import * as sendops from "../lib/sendops";

const t = (k: string, o?: Record<string, unknown>) => i18n.t(k, o) as string;

// ── store snapshots for reset ───────────────────────────────────────────────
const inbox0 = { ...useInbox.getState() };
const data0 = { ...useData.getState() };
const prefs0 = { ...usePrefs.getState() };
const sysDark0 = { ...useSysDark.getState() };
const toast0 = { ...useToast.getState() };
const overlays0 = { ...useOverlays.getState() };

const device = (over: Partial<DiscoveredDevice> = {}): DiscoveredDevice => ({
  deviceId: "dev-1",
  name: "Studio Mac",
  address: "192.168.1.9",
  port: 51704,
  ...over,
});

const fileItem = (over: Partial<InboxItem> = {}): InboxItem => ({
  id: "f1",
  kind: "img",
  ext: "PNG",
  name: "vacation.png",
  from: "Bob",
  ts: Date.now(),
  sizeBytes: 2048,
  count: 1,
  paths: ["/dl/vacation.png"],
  ...over,
});

const textItem = (over: Partial<InboxItem> = {}): InboxItem => ({
  id: "t1",
  kind: "txt",
  ext: "TXT",
  name: "remember the milk",
  from: "Alice",
  ts: Date.now(),
  sizeBytes: 0,
  count: 1,
  text: "remember the milk",
  ...over,
});

/** Reveal a row's hover action chips (they only mount on mouseenter). */
function hoverRow(name: string): void {
  const row = screen.getByText(name).closest("div");
  expect(row).not.toBeNull();
  fireEvent.mouseEnter(row as HTMLElement);
}

beforeEach(() => {
  useInbox.setState(inbox0, true);
  useData.setState(data0, true);
  usePrefs.setState(prefs0, true);
  useSysDark.setState(sysDark0, true);
  useToast.setState(toast0, true);
  useOverlays.setState(overlays0, true);
  useData.setState({ devices: [], downloadDir: "~/Downloads/LanBeam" });
  vi.clearAllMocks();
});

describe("InboxPage grouping", () => {
  it("buckets items into today / yesterday / earlier by timestamp", () => {
    // Freeze time so whenGroup() is deterministic (noon avoids DST edges).
    vi.useFakeTimers();
    const now = new Date(2026, 6, 13, 12, 0, 0).getTime();
    vi.setSystemTime(now);
    useInbox.setState({
      items: [
        fileItem({ id: "a", name: "today.png", ts: now }),
        fileItem({ id: "b", name: "yday.png", ts: now - 24 * 3600_000 }),
        fileItem({ id: "c", name: "old.png", ts: now - 3 * 24 * 3600_000 }),
      ],
      unread: 0,
    });

    renderUI(<InboxPage />);

    expect(screen.getByText(t("inbox.today"))).toBeInTheDocument();
    expect(screen.getByText(t("inbox.yday"))).toBeInTheDocument();
    expect(screen.getByText(t("inbox.earlier"))).toBeInTheDocument();
    expect(screen.getByText("today.png")).toBeInTheDocument();
    expect(screen.getByText("yday.png")).toBeInTheDocument();
    expect(screen.getByText("old.png")).toBeInTheDocument();

    vi.useRealTimers();
  });

  it("shows the empty-state card when nothing matches", () => {
    useInbox.setState({ items: [], unread: 0 });
    renderUI(<InboxPage />);
    expect(screen.getByText(t("inbox.empty"))).toBeInTheDocument();
  });
});

describe("InboxPage category filter", () => {
  it("keeps only text items when the Text segment is chosen", () => {
    useInbox.setState({
      items: [fileItem(), textItem()],
      unread: 0,
    });
    renderUI(<InboxPage />);

    // Both visible under the default "all" filter.
    expect(screen.getByText("vacation.png")).toBeInTheDocument();
    expect(screen.getByText("remember the milk")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: t("inbox.filterTxt") }));

    expect(screen.queryByText("vacation.png")).not.toBeInTheDocument();
    expect(screen.getByText("remember the milk")).toBeInTheDocument();
  });
});

describe("InboxPage row actions", () => {
  it("reveals a file via its non-hover 'show in folder' link", () => {
    useInbox.setState({ items: [fileItem()], unread: 0 });
    renderUI(<InboxPage />);

    fireEvent.click(screen.getByText(t("inbox.showPos")));

    expect(sendops.revealFile).toHaveBeenCalledWith("/dl/vacation.png");
  });

  it("opens a file via the hover 'open' chip", () => {
    useInbox.setState({ items: [fileItem()], unread: 0 });
    renderUI(<InboxPage />);

    hoverRow("vacation.png");
    fireEvent.click(screen.getByText(t("inbox.openAction")));

    expect(sendops.openFile).toHaveBeenCalledWith("/dl/vacation.png");
  });

  it("copies a text item's content via its 'copy' link", () => {
    useInbox.setState({ items: [textItem()], unread: 0 });
    renderUI(<InboxPage />);

    fireEvent.click(screen.getByText(t("inbox.copyAction")));

    expect(sendops.copyText).toHaveBeenCalledWith("remember the milk");
    // and the confirmation toast fired
    expect(useToast.getState().msg).toBe(t("inbox.copiedText"));
  });
});

describe("InboxPage forward flow", () => {
  it("forwards a file to a device through the forward menu", () => {
    useData.setState({ devices: [device()] });
    useInbox.setState({ items: [fileItem()], unread: 0 });
    renderUI(<InboxPage />);

    hoverRow("vacation.png");
    fireEvent.click(screen.getByText(t("common.forward")));
    // The forward submenu lists discovered devices; pick ours.
    fireEvent.click(screen.getByText("Studio Mac"));

    expect(sendops.sendToDevice).toHaveBeenCalledTimes(1);
    const [dev, files] = (
      sendops.sendToDevice as unknown as {
        mock: { calls: [DiscoveredDevice, { name: string }[]][] };
      }
    ).mock.calls[0];
    expect(dev.deviceId).toBe("dev-1");
    expect(files.map((f) => f.name)).toEqual(["vacation.png"]);
  });

  it("forwards a text item over the tracked text channel", () => {
    useData.setState({ devices: [device()] });
    useInbox.setState({ items: [textItem()], unread: 0 });
    renderUI(<InboxPage />);

    hoverRow("remember the milk");
    fireEvent.click(screen.getByText(t("common.forward")));
    fireEvent.click(screen.getByText("Studio Mac"));

    expect(sendops.sendTextTracked).toHaveBeenCalledWith(
      "dev-1",
      "Studio Mac",
      "remember the milk",
      true,
    );
    expect(sendops.sendToDevice).not.toHaveBeenCalled();
  });
});
