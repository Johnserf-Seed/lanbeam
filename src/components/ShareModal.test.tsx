// Component tests for ShareModal (M8.2 「用浏览器接收」). The env is non-Tauri, so
// the bridge share commands take their browser-mode stubs: startShare resolves a
// demo 127.0.0.1 link, listShares returns [], stop/update resolve. We spy on
// those three commands (delegating to the real stubs) plus copyText to assert the
// create → copy → reconfigure → stop flow. Text is queried via i18n.t(key) so the
// assertions don't hard-code a resolved language.
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import type { SendFile } from "../lib/store";
import { useOverlays, useToast } from "../lib/store";
import { fireEvent, renderUI, screen, waitFor } from "../test/render";
import ShareModal from "./ShareModal";

// Spy on the share bridge commands but keep the real browser-mode behavior
// (isTauri stays false, so the stubs return the demo link / empty list).
vi.mock("../bridge/api", async () => {
  const actual =
    await vi.importActual<typeof import("../bridge/api")>("../bridge/api");
  return {
    ...actual,
    startShare: vi.fn(actual.startShare),
    stopShare: vi.fn(actual.stopShare),
    updateShare: vi.fn(actual.updateShare),
    listShares: vi.fn(actual.listShares),
  };
});

// copyText writes to navigator.clipboard (absent in happy-dom); stub it so the
// copy-link assertion is deterministic without touching the clipboard.
vi.mock("../lib/sendops", async () => {
  const actual =
    await vi.importActual<typeof import("../lib/sendops")>("../lib/sendops");
  return { ...actual, copyText: vi.fn() };
});

import { listShares, startShare, stopShare, updateShare } from "../bridge/api";
import type { ShareEntry } from "../bridge/api";
import { copyText } from "../lib/sendops";

const overlays0 = { ...useOverlays.getState() };
const toast0 = { ...useToast.getState() };

const FILE: SendFile = {
  path: "C:/photos/beach.png",
  name: "beach.png",
  ext: "png",
  size: 2048,
};

/** Seed a send flow whose selection is one path-backed file, open the share
 *  overlay, and render. Without a path-backed selection the modal would take the
 *  browser-demo「no local file server」branch instead of creating a share. */
function seedAndRender() {
  useOverlays.setState({
    send: {
      step: "confirm",
      preset: false,
      deviceIds: [],
      pool: [FILE],
      sel: [FILE.path as string],
      pending: [],
      startedTrusted: 0,
    },
    shareOpen: true,
  });
  return renderUI(<ShareModal />);
}

/** Render + wait until the created share link is on screen. */
async function renderWithLink() {
  const view = seedAndRender();
  await screen.findByText(/127\.0\.0\.1:51705\/s\//);
  return view;
}

beforeEach(() => {
  useOverlays.setState(overlays0, true);
  useToast.setState(toast0, true);
  vi.clearAllMocks();
});

describe("ShareModal", () => {
  it("renders nothing when the overlay is closed", () => {
    useOverlays.setState({ shareOpen: false });
    const { container } = renderUI(<ShareModal />);
    expect(container).toBeEmptyDOMElement();
    expect(startShare).not.toHaveBeenCalled();
  });

  it("creates a share for the selected file and shows the link + copy + stop", async () => {
    await renderWithLink();

    // One share registered for the selected path with the default TTL (10 min =
    // 600s) and download cap (1).
    expect(startShare).toHaveBeenCalledTimes(1);
    expect(startShare).toHaveBeenCalledWith([FILE.path], 600, 1);

    // Header + the affordances that only exist once the link is live.
    expect(screen.getByText(i18n.t("share.title"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("share.copyLink"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("share.stop"))).toBeInTheDocument();
    // The LAN reassurance note (softens the browser "not secure" warning) is
    // shown alongside the live link.
    expect(screen.getByText(i18n.t("share.lanNote"))).toBeInTheDocument();
    // The live download count starts at zero and is shown with the link.
    expect(
      screen.getByText(i18n.t("share.dlCount", { n: 0 })),
    ).toBeInTheDocument();
    // The demo/no-server note must NOT show on a successful create.
    expect(screen.queryByText(i18n.t("share.demoNote"))).toBeNull();
  });

  it("copies the link and toasts on 'copy link'", async () => {
    await renderWithLink();

    fireEvent.click(screen.getByText(i18n.t("share.copyLink")));

    expect(copyText).toHaveBeenCalledTimes(1);
    expect(copyText).toHaveBeenCalledWith(
      expect.stringContaining("127.0.0.1:51705/s/"),
    );
    expect(useToast.getState().msg).toBe(i18n.t("share.copiedToast"));
  });

  it("shows TTL + max-download controls and reconfigures the live share", async () => {
    await renderWithLink();

    // Defaults: "Valid 10 min" lifetime pill + the "1 download" cap pill.
    const lifePill = screen.getByText(
      i18n.t("share.life", { t: i18n.t("share.life10m") }),
    );
    expect(lifePill).toBeInTheDocument();
    expect(screen.getByText(i18n.t("share.once1"))).toBeInTheDocument();

    // Open the lifetime dropdown and pick "1 hour" → reconfigure to 3600s, cap 1.
    fireEvent.click(lifePill);
    fireEvent.click(screen.getByText(i18n.t("share.life1h")));
    await waitFor(() => {
      expect(updateShare).toHaveBeenCalledWith(expect.any(String), 3600, 1);
    });

    // Open the download-count dropdown (still showing the default "1 download"
    // pill) and pick "Unlimited" → maxDownloads null.
    fireEvent.click(screen.getByText(i18n.t("share.once1")));
    fireEvent.click(screen.getByText(i18n.t("share.onceAny")));
    await waitFor(() => {
      expect(updateShare).toHaveBeenCalledWith(
        expect.any(String),
        expect.any(Number),
        null,
      );
    });
  });

  it("stops the share, closes the overlay, and toasts on 'stop sharing'", async () => {
    await renderWithLink();

    fireEvent.click(screen.getByText(i18n.t("share.stop")));

    expect(stopShare).toHaveBeenCalledTimes(1);
    expect(stopShare).toHaveBeenCalledWith(expect.any(String));
    expect(useOverlays.getState().shareOpen).toBe(false);
    expect(useToast.getState().msg).toBe(i18n.t("share.stoppedToast"));
  });

  it("shows the browser-demo note when there is no path-backed selection", async () => {
    // No send flow → nothing path-backed to serve → the honest demo note, and no
    // share is ever registered.
    useOverlays.setState({ send: null, shareOpen: true });
    renderUI(<ShareModal />);

    expect(
      await screen.findByText(i18n.t("share.demoNote")),
    ).toBeInTheDocument();
    expect(startShare).not.toHaveBeenCalled();
    expect(screen.queryByText(i18n.t("share.stop"))).toBeNull();
  });

  describe("a share you left running", () => {
    // The bug this whole block exists for: closing the panel does NOT stop the
    // share (a link you handed someone shouldn't die because you closed the panel
    // you copied it from) — but it used to do that SILENTLY, and reopening the
    // panel minted a NEW share instead of picking up the live one. So a forgotten
    // share went on serving files over HTTP, invisible and unstoppable: the only
    // 停止分享 button in the app now pointed at a different share.
    const LIVE: ShareEntry = {
      token: "livetoken",
      url: "http://192.168.1.5:51705/s/livetoken",
      fileCount: 2,
      totalSize: 4096,
      expiresAt: Date.now() / 1000 + 600,
      downloads: 0,
      maxDownloads: 1,
    };

    it("says the link is still live when you close the panel", async () => {
      await renderWithLink();

      fireEvent.click(screen.getByRole("button", { name: "×" }));

      expect(useOverlays.getState().shareOpen).toBe(false);
      // Not silence. The user closed a box, not a share.
      expect(useToast.getState().msg).toBe(i18n.t("share.keptToast"));
      expect(stopShare).not.toHaveBeenCalled();
    });

    it("picks the running share back up instead of starting another one", async () => {
      vi.mocked(listShares).mockResolvedValueOnce([LIVE]);
      // Opened bare (no send-flow selection) — i.e. from the sidebar's live
      // indicator, which is the only way back to a share you closed on.
      useOverlays.setState({ send: null, shareOpen: true });
      renderUI(<ShareModal />);

      await screen.findByText(/192\.168\.1\.5:51705\/s\/livetoken/);
      // The whole point: it did NOT mint a second share.
      expect(startShare).not.toHaveBeenCalled();
      expect(useToast.getState().msg).toBe(i18n.t("share.adoptToast"));
    });

    it("still starts a NEW share when the send flow hands it files", async () => {
      // Adopting must not hijack "share THESE files".
      vi.mocked(listShares).mockResolvedValueOnce([LIVE]);
      await renderWithLink();
      expect(startShare).toHaveBeenCalledTimes(1);
    });
  });
});
