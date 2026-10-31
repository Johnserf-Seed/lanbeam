// Component tests for the send-flow modal (SendModal). The env is non-Tauri, so
// the real sendops.beginSend would return null (no backend) — we mock it so
// the confirm→waiting transition is drivable. Text is asserted via i18n.t(key)
// so the tests don't hard-code a resolved language.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import i18n from "../i18n";
import {
  useData,
  useOverlays,
  usePrefs,
  useToast,
  useTransfers,
  useTrust,
} from "../lib/store";
import type { SendFile } from "../lib/store";
import { fireEvent, renderUI, screen, waitFor } from "../test/render";
import SendModal from "./SendModal";

// beginSend is the real send entrypoint the confirm step fires; mock it so a
// click can drive the flow to the waiting step (browser mode would bail early).
// It returns the session id the UI chose for that send.
vi.mock("../lib/sendops", () => ({
  beginSend: vi.fn(() => "sess-1"),
  pickFiles: vi.fn(async () => [] as SendFile[]),
}));
import { beginSend } from "../lib/sendops";
// The waiting step's 取消未确认的请求 cancels for real (M20); observe it.
vi.mock("../bridge/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../bridge/api")>()),
  cancelTransfer: vi.fn(async () => {}),
}));
import { cancelTransfer } from "../bridge/api";

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
  vi.mocked(beginSend).mockClear().mockReturnValue("sess-1");
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

  it("fires beginSend and moves to the waiting step on Start", async () => {
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
    expect(vi.mocked(beginSend)).toHaveBeenCalledTimes(1);
    // called with the target device and the persisted stripExif choice
    const [dev, files, strip] = vi.mocked(beginSend).mock.calls[0];
    expect(dev.deviceId).toBe("demo-mini");
    expect(files.map((f) => f.name)).toEqual(["alpha.txt"]);
    expect(strip).toBe(usePrefs.getState().stripExif);
    // one untrusted pending row, tied to the session that send started
    expect(useOverlays.getState().send?.pending).toEqual([
      expect.objectContaining({ deviceId: "demo-mini", sessionId: "sess-1" }),
    ]);
  });
});

describe("SendModal · the waiting step", () => {
  /** Park the modal on the waiting step for one untrusted device. */
  const waitOn = (sessionId = "sess-1") => {
    useData.setState({ devices: [mini] });
    useOverlays.getState().openSend("demo-mini", [fileA], [fileA]);
    useOverlays.getState().patchSend({
      step: "waiting",
      pending: [{ deviceId: "demo-mini", name: "Mini", sessionId, ok: false }],
    });
    renderUI(<SendModal />);
  };

  it("stays open with the SAS while the receiver decides — and closes when they accept", async () => {
    // H10: the connection opening (transfer_started) used to count as "they
    // accepted", closing this step before anyone could compare the SAS.
    waitOn();
    useTransfers.getState().upsert({
      sessionId: "sess-1",
      direction: "send",
      status: "active",
      started: true,
      sas: "483921",
    });
    expect(await screen.findByText("483 · 921")).toBeInTheDocument();
    expect(useOverlays.getState().send?.step).toBe("waiting");

    useTransfers.getState().upsert({ sessionId: "sess-1", accepted: true });
    await waitFor(() => expect(useOverlays.getState().send).toBeNull());
    // And the toast says what happened — accepted — not that a code was
    // compared, which the sender has no way to know.
    expect(useToast.getState().msg).toBe(
      i18n.t("send.sentOneToast", { name: "Mini" }),
    );
  });

  it("取消未确认的请求 cancels the requests still waiting — not just the window (M20)", async () => {
    useData.setState({ devices: [mini] });
    useOverlays.getState().openSend("demo-mini", [fileA], [fileA]);
    useOverlays.getState().patchSend({
      step: "waiting",
      pending: [
        { deviceId: "demo-mini", name: "Mini", sessionId: "waits", ok: false },
        { deviceId: "demo-mini", name: "Mini", sessionId: "took", ok: false },
      ],
    });
    useTransfers.getState().upsert({
      sessionId: "took",
      direction: "send",
      status: "active",
      accepted: true,
    });
    renderUI(<SendModal />);

    fireEvent.click(screen.getByText(i18n.t("send.cancelWaiting")));
    expect(cancelTransfer).toHaveBeenCalledTimes(1);
    expect(cancelTransfer).toHaveBeenCalledWith("waits");
    expect(useOverlays.getState().send).toBeNull();
    expect(useToast.getState().msg).toBe(i18n.t("send.waitingCanceledToast"));
  });

  it("follows ITS session, not the newest send to the same device", async () => {
    // H11: rows used to be matched by device and start time.
    waitOn("sess-1");
    useTransfers.getState().upsert({
      sessionId: "other",
      direction: "send",
      status: "active",
      peerId: "demo-mini",
      accepted: true,
    });
    await new Promise((r) => setTimeout(r, 0));
    expect(useOverlays.getState().send?.step).toBe("waiting");
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

describe("SendModal · closing keeps or drops the selection", () => {
  it("a click outside keeps it: reopening resumes at the same step", () => {
    useData.setState({ devices: [mini, nano] });
    useOverlays.getState().openSend(null, [fileA, fileB]);
    useOverlays.getState().patchSend({ sel: ["beta.pdf"], step: "device" });
    const { container } = renderUI(<SendModal />);

    const scrim = container.querySelector(".scrim") as HTMLElement;
    fireEvent.mouseDown(scrim);
    fireEvent.click(scrim);
    expect(useOverlays.getState().send).toBeNull();

    // reopened the plain way (the header's 发送文件 button)
    useOverlays.getState().openSend(null, [fileA, fileB]);
    expect(useOverlays.getState().send?.step).toBe("device");
    expect(useOverlays.getState().send?.sel).toEqual(["beta.pdf"]);
  });

  it("Escape is a click outside: it keeps the selection too", () => {
    useData.setState({ devices: [mini, nano] });
    useOverlays.getState().openSend(null, [fileA, fileB]);
    useOverlays.getState().patchSend({ sel: ["beta.pdf"], step: "device" });
    renderUI(<SendModal />);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(useOverlays.getState().send).toBeNull();
    expect(useOverlays.getState().sendDraft?.sel).toEqual(["beta.pdf"]);
  });

  it("the × drops it: reopening starts over", () => {
    useData.setState({ devices: [mini, nano] });
    useOverlays.getState().openSend(null, [fileA, fileB]);
    useOverlays.getState().patchSend({ sel: ["beta.pdf"], step: "device" });
    renderUI(<SendModal />);

    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("common.close") }),
    );
    expect(useOverlays.getState().send).toBeNull();
    expect(useOverlays.getState().sendDraft).toBeNull();

    useOverlays.getState().openSend(null, [fileA, fileB]);
    expect(useOverlays.getState().send?.step).toBe("files");
    expect(useOverlays.getState().send?.sel).toEqual([]);
  });
});
