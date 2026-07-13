// Component tests for QuickTextModal — the ⌁ quick-text overlay (M7.3). The
// modal only mounts when useOverlays.qtOpen is set, so each test seeds that flag
// plus a device in useData, then drives the textarea + send button. The real
// sendTextTracked hits the (browser-mode) bridge, so it's mocked here to assert
// the send wiring without a live backend.
import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import i18n from "../i18n";
import type { DiscoveredDevice } from "../bridge/api";
import { useData, useOverlays } from "../lib/store";
import { renderUI, screen, waitFor } from "../test/render";
import QuickTextModal from "./QuickTextModal";

// Spy on the send path; errText is only hit on failure — keep a plain passthrough.
const sendTextTracked = vi.fn<
  (id: string, name: string, text: string, clip: boolean) => Promise<void>
>(() => Promise.resolve());
vi.mock("../lib/sendops", () => ({
  sendTextTracked: (...args: [string, string, string, boolean]) =>
    sendTextTracked(...args),
  errText: (e: unknown) => String(e),
}));

const device = (over: Partial<DiscoveredDevice> = {}): DiscoveredDevice => ({
  deviceId: "dev-1",
  name: "Studio Mac",
  address: "192.168.1.30",
  port: 51704,
  ...over,
});

// Clean overlay + data slices before each test so cases can't leak into one
// another; the modal's own open-effect resets its draft fields.
const overlays0 = { ...useOverlays.getState() };
const data0 = { ...useData.getState() };

beforeEach(() => {
  sendTextTracked.mockClear();
  useOverlays.setState(overlays0, true);
  useData.setState(data0, true);
});

describe("QuickTextModal", () => {
  it("renders nothing while closed", () => {
    const { container } = renderUI(<QuickTextModal />);
    expect(container.querySelector(".scrim")).toBeNull();
  });

  it("renders the textarea + target picker when open with a device", () => {
    useData.setState({ devices: [device()] });
    useOverlays.setState({ qtOpen: true });
    renderUI(<QuickTextModal />);

    // textarea by its placeholder
    expect(screen.getByPlaceholderText(i18n.t("qt.placeholder"))).toBeTruthy();
    // target picker lists the seeded device
    const select = screen.getByRole("combobox") as HTMLSelectElement;
    expect(select).toBeTruthy();
    expect(screen.getByRole("option", { name: "Studio Mac" })).toBeTruthy();
    // the send button renders
    expect(
      screen.getByRole("button", { name: i18n.t("qt.send") }),
    ).toBeTruthy();
  });

  it("sends the trimmed text to the selected device and closes", async () => {
    const user = userEvent.setup();
    useData.setState({ devices: [device()] });
    useOverlays.setState({ qtOpen: true });
    renderUI(<QuickTextModal />);

    await user.type(
      screen.getByPlaceholderText(i18n.t("qt.placeholder")),
      "  hello lan  ",
    );
    await user.click(screen.getByRole("button", { name: i18n.t("qt.send") }));

    await waitFor(() => expect(sendTextTracked).toHaveBeenCalledTimes(1));
    // targetId + targetName default to the first device; text is trimmed; the
    // clipboard toggle defaults on (true).
    expect(sendTextTracked).toHaveBeenCalledWith(
      "dev-1",
      "Studio Mac",
      "hello lan",
      true,
    );
    // a successful send closes the modal
    await waitFor(() => expect(useOverlays.getState().qtOpen).toBe(false));
  });

  it("is a no-op when the text is empty (or only whitespace)", async () => {
    const user = userEvent.setup();
    useData.setState({ devices: [device()] });
    useOverlays.setState({ qtOpen: true });
    renderUI(<QuickTextModal />);

    // click send with an empty draft
    await user.click(screen.getByRole("button", { name: i18n.t("qt.send") }));
    expect(sendTextTracked).not.toHaveBeenCalled();

    // whitespace-only is also rejected by the trim() guard
    await user.type(
      screen.getByPlaceholderText(i18n.t("qt.placeholder")),
      "   ",
    );
    await user.click(screen.getByRole("button", { name: i18n.t("qt.send") }));
    expect(sendTextTracked).not.toHaveBeenCalled();

    // still open — nothing sent
    expect(useOverlays.getState().qtOpen).toBe(true);
  });
});
