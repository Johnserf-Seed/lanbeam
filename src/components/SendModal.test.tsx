// Component tests for the send-flow modal (SendModal). The env is non-Tauri, so
// the real sendops.sendToDevice would return false (no backend) — we mock it so
// the confirm→waiting transition is drivable. Text is asserted via i18n.t(key)
// so the tests don't hard-code a resolved language.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import i18n from "../i18n";
import {
  useData,
  useOverlays,
  usePrefs,
  useTransfers,
  useTrust,
} from "../lib/store";
import type { SendFile } from "../lib/store";
import { fireEvent, renderUI, screen, waitFor } from "../test/render";
import SendModal from "./SendModal";

// sendToDevice is the real send entrypoint the confirm step fires; mock it so a
// click can drive the flow to the waiting step (browser mode would bail early).
vi.mock("../lib/sendops", () => ({
  sendToDevice: vi.fn(() => true),
  pickFiles: vi.fn(async () => [] as SendFile[]),
}));
import { sendToDevice } from "../lib/sendops";

// ── clean-state snapshots ────────────────────────────────────────────────────
const overlays0 = { ...useOverlays.getState() };
const data0 = { ...useData.getState() };
const trust0 = { ...useTrust.getState() };
const prefs0 = { ...usePrefs.getState() };
const transfers0 = { ...useTransfers.getState() };

const mini: DiscoveredDevice = {
  deviceId: "demo-mini",
  name: "Mini",
  address: "192.168.1.31",
  port: 51704,
};
const nano: DiscoveredDevice = {
  deviceId: "demo-nano",
  name: "Nano",
  address: "192.168.1.32",
  port: 51704,
};

const fileA: SendFile = { name: "alpha.txt", ext: "TXT", size: 1024 };
const fileB: SendFile = { name: "beta.pdf", ext: "PDF", size: 2048 };

beforeEach(() => {
  useOverlays.setState(overlays0, true);
  useData.setState(data0, true);
  useTrust.setState(trust0, true);
  usePrefs.setState(prefs0, true);
  useTransfers.setState(transfers0, true);
  vi.mocked(sendToDevice).mockClear().mockReturnValue(true);
});

describe("SendModal · files step", () => {
  it("renders the file pool and the preset target device", () => {
    useData.setState({ devices: [mini] });
    // preset device + preselected file → files step, sub names the target
    useOverlays.getState().openSend("demo-mini", [fileA, fileB], [fileA]);
    renderUI(<SendModal />);

    // both pooled files render
    expect(screen.getByText("alpha.txt")).toBeTruthy();
    expect(screen.getByText("beta.pdf")).toBeTruthy();
    // the target device name shows in the header sub
    expect(
      screen.getByText(i18n.t("send.subFilesTo", { name: "Mini" })),
    ).toBeTruthy();
  });

  it("toggles a file's selection when its row is clicked", () => {
    useData.setState({ devices: [mini] });
    // no presel → nothing selected initially
    useOverlays.getState().openSend("demo-mini", [fileA, fileB]);
    renderUI(<SendModal />);

    expect(useOverlays.getState().send?.sel).toEqual([]);

    fireEvent.click(screen.getByText("alpha.txt"));
    expect(useOverlays.getState().send?.sel).toContain("alpha.txt");

    // clicking again deselects
    fireEvent.click(screen.getByText("alpha.txt"));
    expect(useOverlays.getState().send?.sel).not.toContain("alpha.txt");
  });
});

describe("SendModal · step transitions", () => {
  it("advances files → device when no device is preset", async () => {
    useData.setState({ devices: [mini, nano] });
    // preset=false (null device), preselected file so Next is enabled
    useOverlays.getState().openSend(null, [fileA], [fileA]);
    renderUI(<SendModal />);

    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("common.next") }),
    );

    await waitFor(() =>
      expect(useOverlays.getState().send?.step).toBe("device"),
    );
    // device cards now render both live devices
    expect(screen.getByText("Mini")).toBeTruthy();
    expect(screen.getByText("Nano")).toBeTruthy();
  });

  it("advances files → confirm when a device is preset", async () => {
    useData.setState({ devices: [mini] });
    useOverlays.getState().openSend("demo-mini", [fileA], [fileA]);
    renderUI(<SendModal />);

    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("common.next") }),
    );

    await waitFor(() =>
      expect(useOverlays.getState().send?.step).toBe("confirm"),
    );
    expect(screen.getByText(i18n.t("send.titleConfirm"))).toBeTruthy();
  });

  it("fires sendToDevice and moves to the waiting step on Start", async () => {
    useData.setState({ devices: [mini] });
    useOverlays.getState().openSend("demo-mini", [fileA], [fileA]);
    renderUI(<SendModal />);

    // files → confirm
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("common.next") }),
    );
    await waitFor(() =>
      expect(useOverlays.getState().send?.step).toBe("confirm"),
    );

    // confirm → Start (device is untrusted, so the flow parks on waiting)
    fireEvent.click(
      screen.getByRole("button", { name: new RegExp(i18n.t("send.start")) }),
    );

    await waitFor(() =>
      expect(useOverlays.getState().send?.step).toBe("waiting"),
    );
    expect(vi.mocked(sendToDevice)).toHaveBeenCalledTimes(1);
    // called with the target device and the persisted stripExif choice
    const [dev, files, strip] = vi.mocked(sendToDevice).mock.calls[0];
    expect(dev.deviceId).toBe("demo-mini");
    expect(files.map((f) => f.name)).toEqual(["alpha.txt"]);
    expect(strip).toBe(usePrefs.getState().stripExif);
    // one untrusted pending row was queued
    expect(useOverlays.getState().send?.pending).toHaveLength(1);
  });
});

describe("SendModal · scrim drag-guard", () => {
  it("does NOT close when a press starts inside the modal and ends on the scrim", () => {
    useData.setState({ devices: [mini] });
    useOverlays.getState().openSend("demo-mini", [fileA], [fileA]);
    const { container } = renderUI(<SendModal />);

    const scrim = container.querySelector(".scrim") as HTMLElement;
    const modal = container.querySelector(".modal") as HTMLElement;
    expect(scrim).toBeTruthy();
    expect(modal).toBeTruthy();

    // mousedown inside the modal → scrim's onMouseDown sees target !== scrim
    fireEvent.mouseDown(modal);
    // click bubbles to the scrim, but the guard flag is false → no close
    fireEvent.click(scrim);

    expect(useOverlays.getState().send).not.toBeNull();
  });

  it("closes on a genuine scrim press-and-release", () => {
    useData.setState({ devices: [mini] });
    useOverlays.getState().openSend("demo-mini", [fileA], [fileA]);
    const { container } = renderUI(<SendModal />);

    const scrim = container.querySelector(".scrim") as HTMLElement;
    // press and release both land on the scrim itself → guard flag true
    fireEvent.mouseDown(scrim);
    fireEvent.click(scrim);

    expect(useOverlays.getState().send).toBeNull();
  });
});
