// Component tests for PairModal. The env is non-Tauri, so api.startPairing /
// joinByCode return the browser-mode demo stubs (a static code + lanbeam://
// deep-link QR payload). We seed useOverlays.pairOpen (and pairPrefill) before
// rendering and reset the overlay + toast stores between tests. Text is queried
// through i18n.t(key) so the assertions hold in whatever language i18n resolves.
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import * as sendops from "../lib/sendops";
import { useOverlays, useToast } from "../lib/store";
import { fireEvent, renderUI, screen, waitFor } from "../test/render";
import PairModal from "./PairModal";

// Spy on copyText but keep the rest of sendops (errText etc.) real.
vi.mock("../lib/sendops", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/sendops")>();
  return { ...actual, copyText: vi.fn() };
});

// The browser-mode start_pairing stub QR payload (see bridge/api.ts).
const DEMO_QR =
  "lanbeam://pair?d=demo&n=%E4%B9%A6%E6%88%BF&a=192.168.1.20&p=51704&c=482913";

const overlays0 = { ...useOverlays.getState() };
const toast0 = { ...useToast.getState() };

beforeEach(() => {
  useOverlays.setState(overlays0, true);
  useToast.setState(toast0, true);
  vi.mocked(sendops.copyText).mockClear();
});

describe("PairModal", () => {
  it("renders nothing while closed", () => {
    const { container } = renderUI(<PairModal />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the host code, QR and the join field when open", async () => {
    useOverlays.setState({ pairOpen: true });
    renderUI(<PairModal />);

    // The joiner section's address field is present immediately.
    expect(
      screen.getByPlaceholderText(i18n.t("pair.joinAddr")),
    ).toBeInTheDocument();
    // Static UI labels render.
    expect(screen.getByText(i18n.t("pair.orCode"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("pair.waiting"))).toBeInTheDocument();

    // start_pairing resolves the demo code (grouped 3+3 by fmtCode) and QR.
    await waitFor(() =>
      expect(screen.getByText("482 913")).toBeInTheDocument(),
    );
    expect(await screen.findByRole("img", { name: "QR" })).toBeInTheDocument();
  });

  it("pre-fills the join field from a deep-link prefill and toasts", () => {
    const link = "lanbeam://pair?d=x&a=10.0.0.9&c=123456";
    useOverlays.setState({ pairOpen: true, pairPrefill: link });
    renderUI(<PairModal />);

    const addr = screen.getByPlaceholderText(
      i18n.t("pair.joinAddr"),
    ) as HTMLInputElement;
    expect(addr.value).toBe(link);
    // The prefill is consumed once so a later manual reopen starts clean.
    expect(useOverlays.getState().pairPrefill).toBeNull();
    // A cue toast fires telling the user to review + Join.
    expect(useToast.getState().msg).toBe(i18n.t("pair.linkLoaded"));
  });

  it("copies the pairing link and toasts when the QR is clicked", async () => {
    useOverlays.setState({ pairOpen: true });
    renderUI(<PairModal />);

    const qr = await screen.findByRole("img", { name: "QR" });
    // The click handler lives on the div wrapping <Qr>.
    const clickable = qr.parentElement as HTMLElement;
    fireEvent.click(clickable);

    expect(sendops.copyText).toHaveBeenCalledWith(DEMO_QR);
    expect(useToast.getState().msg).toBe(i18n.t("pair.linkCopied"));
  });

  it("closes the modal from the header close button", () => {
    useOverlays.setState({ pairOpen: true });
    renderUI(<PairModal />);
    expect(screen.getByText(i18n.t("pair.title"))).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "×" }));
    expect(useOverlays.getState().pairOpen).toBe(false);
    expect(screen.queryByText(i18n.t("pair.title"))).not.toBeInTheDocument();
  });
});
