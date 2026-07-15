// Component tests for PairModal. The env is non-Tauri, so api.startPairing /
// joinByCode return the browser-mode demo stubs (a static code + lanbeam://
// deep-link QR payload). We seed useOverlays.pairOpen (and pairPrefill) before
// rendering and reset the overlay + toast stores between tests. Text is queried
// through i18n.t(key) so the assertions hold in whatever language i18n resolves.
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import * as sendops from "../lib/sendops";
import { useOverlays, useToast, useTrust } from "../lib/store";
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
const trust0 = { ...useTrust.getState() };

beforeEach(() => {
  useOverlays.setState(overlays0, true);
  useToast.setState(toast0, true);
  useTrust.setState({ ...trust0, records: {} }, true);
  vi.mocked(sendops.copyText).mockClear();
});

/** Drive the joiner half to the compare step. The browser-mode join_by_code stub
 *  resolves { deviceId: "demo-paired", name: "Pixel 8 Pro", sas: "483921" }. */
const joinToCompare = async () => {
  fireEvent.change(screen.getByPlaceholderText(i18n.t("pair.joinAddr")), {
    target: { value: "192.168.1.20" },
  });
  fireEvent.change(screen.getByPlaceholderText(i18n.t("pair.joinCode")), {
    target: { value: "482913" },
  });
  fireEvent.click(screen.getByRole("button", { name: i18n.t("pair.joinBtn") }));
  await screen.findByText(i18n.t("pair.match"));
};

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

  describe("the SAS compare step", () => {
    it("shows the joiner the SAS instead of quietly closing", async () => {
      // The whole point: the side that redeems the code sees the SAME number the
      // host is showing. A code only one end can read is not a check at all.
      useOverlays.setState({ pairOpen: true });
      renderUI(<PairModal />);
      await joinToCompare();

      expect(screen.getByText("483 · 921")).toBeInTheDocument(); // fmtSas
      expect(screen.getByText(i18n.t("pair.compareWarn"))).toBeInTheDocument();
    });

    it("grants NO trust until the user says the two screens agree", async () => {
      useOverlays.setState({ pairOpen: true });
      renderUI(<PairModal />);
      await joinToCompare();

      // Redeeming a valid code proves the code, not the peer.
      expect(useTrust.getState().records["demo-paired"]).toBeUndefined();

      fireEvent.click(
        screen.getByRole("button", { name: i18n.t("pair.match") }),
      );
      const rec = useTrust.getState().records["demo-paired"];
      // Trust rides the same path as the trust circle — so it comes with
      // auto-accept, exactly like a device dragged into the ring.
      expect(rec).toMatchObject({ trusted: true, autoAccept: true });
      expect(useOverlays.getState().pairOpen).toBe(false);
    });

    it("trusts nothing when the codes don't match, and says why", async () => {
      useOverlays.setState({ pairOpen: true });
      renderUI(<PairModal />);
      await joinToCompare();

      fireEvent.click(
        screen.getByRole("button", { name: i18n.t("pair.mismatch") }),
      );
      expect(useTrust.getState().records["demo-paired"]).toBeUndefined();
      expect(useToast.getState().msg).toBe(i18n.t("pair.mismatchToast"));
    });

    it("trusts nothing when the compare step is dismissed, and doesn't do it silently", async () => {
      // Walking away is a "no". The dangerous version of this bug is the quiet
      // one: the user closes the box believing they paired.
      useOverlays.setState({ pairOpen: true });
      renderUI(<PairModal />);
      await joinToCompare();

      fireEvent.click(screen.getByRole("button", { name: "×" }));
      expect(useTrust.getState().records["demo-paired"]).toBeUndefined();
      expect(useToast.getState().msg).toBe(i18n.t("pair.unconfirmedToast"));
    });
  });
});
