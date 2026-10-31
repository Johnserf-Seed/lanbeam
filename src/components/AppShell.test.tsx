// AppShell is where the backend's events land, and it had no tests. These
// capture the handlers it registers (api.onEvent, mocked) and drive them by
// hand: the rest of the bridge stays its browser-mode self.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  DiscoveredDevice,
  IncomingRequest,
  Settings,
} from "../bridge/api";
import i18n from "../i18n";
import {
  useData,
  useOverlays,
  usePrefs,
  useToast,
  useTransfers,
} from "../lib/store";
import { act, renderUI, waitFor } from "../test/render";

const handlers = vi.hoisted(() => new Map<string, (p: unknown) => void>());
const settingsGate = vi.hoisted(() => ({
  hold: false,
  release: (_s: Settings) => {},
}));
vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return {
    ...actual,
    onEvent: vi.fn((name: string, cb: (p: unknown) => void) => {
      handlers.set(name, cb);
      return () => handlers.delete(name);
    }),
    // A test can hold get_settings to put the startup load in flight.
    getSettings: vi.fn(() =>
      settingsGate.hold
        ? new Promise<Settings>((resolve) => {
            settingsGate.release = resolve;
          })
        : actual.getSettings(),
    ),
    setDiscoverable: vi.fn(actual.setDiscoverable),
    pendingRequests: vi.fn(actual.pendingRequests),
  };
});
import * as api from "../bridge/api";
import AppShell from "./AppShell";

const fire = (name: string, payload: unknown) =>
  act(() => {
    const h = handlers.get(name);
    if (!h) throw new Error(`AppShell registered no ${name} handler`);
    h(payload);
  });

const studio: DiscoveredDevice = {
  deviceId: "dev-studio",
  name: "Studio",
  address: "192.168.1.30",
  port: 51704,
};

const data0 = { ...useData.getState() };
const overlays0 = { ...useOverlays.getState() };
const prefs0 = { ...usePrefs.getState() };
const toast0 = { ...useToast.getState() };
const transfers0 = { ...useTransfers.getState() };

beforeEach(() => {
  handlers.clear();
  settingsGate.hold = false;
  useData.setState(data0, true);
  useOverlays.setState(overlays0, true);
  usePrefs.setState(prefs0, true);
  useToast.setState(toast0, true);
  useTransfers.setState(transfers0, true);
  vi.mocked(api.setDiscoverable).mockClear();
});

/** Mount, then wait out the startup load so it can't land mid-test. */
async function mount() {
  renderUI(<AppShell />);
  await waitFor(() => expect(useData.getState().settings).not.toBeNull());
}

describe("AppShell · transfer events", () => {
  it("a queued send gets its device from the event (M25)", async () => {
    await mount();
    act(() => useData.setState({ devices: [studio] }));
    fire("transfer_queued", {
      sessionId: "q1",
      direction: "send",
      deviceId: "dev-studio",
    });
    expect(useTransfers.getState().transfers.q1).toMatchObject({
      status: "queued",
      peerId: "dev-studio",
      peerName: "Studio",
    });
  });

  it("a request that came in before the window was up is shown once it is (L23)", async () => {
    const early = {
      sessionId: "early",
      deviceId: "dev-studio",
      sas: "123456",
      totalSize: 1,
      fileCount: 1,
      files: [{ name: "a.bin", size: 1 }],
      senderName: "Studio",
      autoAccepted: false,
    } as IncomingRequest;
    vi.mocked(api.pendingRequests).mockResolvedValueOnce([early]);
    await mount();
    await waitFor(() =>
      expect(useTransfers.getState().incomings.map((r) => r.sessionId)).toEqual(
        ["early"],
      ),
    );
    // Its event landing as well doesn't show it twice.
    fire("incoming_file_request", early);
    expect(useTransfers.getState().incomings).toHaveLength(1);
  });

  it("a transfer_done for a session nobody knows makes no row (L22)", async () => {
    await mount();
    fire("transfer_done", { sessionId: "ghost", direction: "receive" });
    expect(useTransfers.getState().transfers.ghost).toBeUndefined();
  });

  it("the user's own cancel isn't reported as a failure; the peer's is (M21)", async () => {
    await mount();
    for (const id of ["mine", "theirs"])
      useTransfers.getState().upsert({
        sessionId: id,
        direction: "send",
        status: "active",
        peerName: "Studio",
      });

    fire("transfer_error", {
      sessionId: "mine",
      error: "cancelled",
      code: "cancelled",
    });
    expect(useTransfers.getState().transfers.mine).toMatchObject({
      status: "error",
      errorCode: "cancelled",
    });
    expect(useToast.getState().msg).toBeNull();

    fire("transfer_error", {
      sessionId: "theirs",
      error: "the peer cancelled the transfer",
      code: "peer_cancelled",
    });
    expect(useToast.getState().msg).toBe(
      i18n.t("transfers.failedToast", {
        peer: "Studio",
        err: i18n.t("errors.peerCancelled"),
      }),
    );
  });

  it("an unanswered collision question is closed when the backend moves on (M6)", async () => {
    await mount();
    const request = {
      sessionId: "k1",
      deviceId: "dev-studio",
      sas: "123456",
      totalSize: 10,
      fileCount: 1,
      files: [{ name: "clash.bin", size: 10 }],
      senderName: "Studio",
      autoAccepted: true,
      conflicts: ["clash.bin"],
      conflictPolicy: "ask",
    } as IncomingRequest;
    act(() =>
      useOverlays
        .getState()
        .setConflict({ request, peerName: "Studio", wantTrust: false }),
    );

    // 120 s unanswered: the backend keeps both and starts the receive.
    fire("transfer_started", {
      sessionId: "k1",
      direction: "receive",
      totalSize: 10,
    });
    expect(useOverlays.getState().conflict).toBeNull();
    expect(useToast.getState().msg).toBe(i18n.t("conflict.timedOutToast"));
    // The row knows what it is, as it would had the question been answered.
    expect(useTransfers.getState().transfers.k1).toMatchObject({
      peerName: "Studio",
      name: "clash.bin",
    });
  });
});

describe("AppShell · ghost mode", () => {
  it("an expired 临时隐身 is restored only after the settings load (M27)", async () => {
    usePrefs.setState({ ghostUntil: Date.now() - 1000 });
    settingsGate.hold = true;
    renderUI(<AppShell />);
    await waitFor(() => expect(api.getSettings).toHaveBeenCalled());
    // The load is in flight: restoring now would be overwritten by it.
    expect(api.setDiscoverable).not.toHaveBeenCalled();

    const real =
      await vi.importActual<typeof import("../bridge/api")>("../bridge/api");
    const base = await real.getSettings();
    // …and it brings the state from before the restore: hidden.
    await act(async () =>
      settingsGate.release({ ...base, discoverable: false }),
    );
    await waitFor(() => expect(api.setDiscoverable).toHaveBeenCalledWith(true));
    expect(useData.getState().settings?.discoverable).toBe(true);
    expect(usePrefs.getState().ghostUntil).toBeNull();
  });
});
