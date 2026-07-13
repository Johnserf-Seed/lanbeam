// Component tests for the devices radar page. The env is non-Tauri, so bridge
// calls take their browser-mode stubs; here we additionally mock the two calls
// the manual IP-connect flow makes so we can assert on them. Each test seeds the
// zustand stores it touches from a captured clean snapshot (see beforeEach) and
// renders via renderUI (wraps in a MemoryRouter — the page uses useNavigate).
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import * as api from "../bridge/api";
import i18n from "../i18n";
import {
  useData,
  useOverlays,
  usePrefs,
  useTransfers,
  useTrust,
} from "../lib/store";
import type { TrustRecord } from "../lib/store";
import { fireEvent, renderUI, screen, waitFor } from "../test/render";
import DevicesPage from "./DevicesPage";

// Mock the bridge so the IP-connect flow's two calls are observable spies while
// every other export (isTauri === false, other stubs) keeps its real value.
vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return {
    ...actual,
    connectByAddr: vi.fn(async (addr: string) => ({
      deviceId: "demo-added",
      name: addr,
      sas: "483921",
    })),
    listDiscoveredDevices: vi.fn(async () => [] as DiscoveredDevice[]),
  };
});

// ── clean-state snapshots (captured once at module load) ────────────────────
const data0 = { ...useData.getState() };
const overlays0 = { ...useOverlays.getState() };
const trust0 = { ...useTrust.getState() };
const transfers0 = { ...useTransfers.getState() };
const prefs0 = { ...usePrefs.getState() };

const dev = (over: Partial<DiscoveredDevice> = {}): DiscoveredDevice => ({
  deviceId: "d1",
  name: "Laptop",
  address: "192.168.1.5",
  port: 51704,
  ...over,
});

const trustRec = (over: Partial<TrustRecord>): TrustRecord => ({
  deviceId: "d1",
  name: "d1",
  trusted: false,
  autoAccept: false,
  addedAt: 1,
  lastSeen: 1,
  ...over,
});

/** Seed the device list without stamping firstSeen, so the「刚刚发现」pulse
 *  note never appears and text assertions stay deterministic. */
const seedDevices = (devices: DiscoveredDevice[]) =>
  useData.setState({ devices, firstSeen: {}, settings: null });

beforeEach(() => {
  vi.clearAllMocks();
  useData.setState(data0, true);
  useOverlays.setState(overlays0, true);
  useTrust.setState(trust0, true);
  useTransfers.setState(transfers0, true);
  usePrefs.setState(prefs0, true);
});

describe("DevicesPage — discovered devices", () => {
  it("renders each discovered device's name and address on the radar", () => {
    seedDevices([
      dev({ deviceId: "d1", name: "Laptop", address: "192.168.1.5" }),
      dev({ deviceId: "d2", name: "Phone", address: "192.168.1.9" }),
    ]);
    renderUI(<DevicesPage />);
    expect(screen.getByText("Laptop")).toBeTruthy();
    expect(screen.getByText("192.168.1.5")).toBeTruthy();
    expect(screen.getByText("Phone")).toBeTruthy();
    expect(screen.getByText("192.168.1.9")).toBeTruthy();
    // the center hub ("this device") always renders
    expect(screen.getByText(i18n.t("devices.self"))).toBeTruthy();
  });

  it("shows the drag hint (not the empty hint) when devices are present", () => {
    seedDevices([dev()]);
    renderUI(<DevicesPage />);
    expect(screen.getByText(i18n.t("devices.dragHint"))).toBeTruthy();
    expect(screen.queryByText(i18n.t("devices.emptyHint"))).toBeNull();
  });

  it("prefers the trusted-record rename over the discovered name", () => {
    seedDevices([dev({ deviceId: "d1", name: "OldName" })]);
    useTrust.setState({
      records: {
        d1: trustRec({ deviceId: "d1", name: "Renamed", trusted: true }),
      },
    });
    renderUI(<DevicesPage />);
    expect(screen.getByText("Renamed")).toBeTruthy();
    expect(screen.queryByText("OldName")).toBeNull();
  });
});

describe("DevicesPage — empty state", () => {
  it("renders the checklist and the manual-connect affordances", () => {
    seedDevices([]);
    renderUI(<DevicesPage />);
    expect(screen.getByText(i18n.t("devices.emptyTitle"))).toBeTruthy();
    expect(screen.getByPlaceholderText("192.168.1.__")).toBeTruthy();
    expect(screen.getByText(i18n.t("devices.ipDirect"))).toBeTruthy();
    expect(screen.getByText(i18n.t("devices.pairInvite"))).toBeTruthy();
    expect(screen.getByText(i18n.t("devices.emptyHint"))).toBeTruthy();
  });

  it("the pair-invite link opens the pairing modal", () => {
    seedDevices([]);
    renderUI(<DevicesPage />);
    expect(useOverlays.getState().pairOpen).toBe(false);
    fireEvent.click(screen.getByText(i18n.t("devices.pairInvite")));
    expect(useOverlays.getState().pairOpen).toBe(true);
  });
});

describe("DevicesPage — clicking a device", () => {
  it("opens the send flow for that device (useOverlays.send seeded)", () => {
    seedDevices([dev({ deviceId: "d1", name: "Laptop" })]);
    const { container } = renderUI(<DevicesPage />);
    expect(useOverlays.getState().send).toBeNull();

    const row = container.querySelector('[data-device-id="d1"]');
    expect(row).not.toBeNull();
    fireEvent.click(row as Element);

    const send = useOverlays.getState().send;
    expect(send).not.toBeNull();
    expect(send?.deviceIds).toEqual(["d1"]);
    expect(send?.preset).toBe(true);
  });

  it("routes a fingerprint-mismatch device to the fp alert, not the send flow", () => {
    // A trusted record ("Alice"/oldkey) is offline; the same display name shows
    // up live under a new key → clicking the live node must open the fp warning.
    seedDevices([dev({ deviceId: "newkey", name: "Alice" })]);
    useTrust.setState({
      records: {
        oldkey: trustRec({ deviceId: "oldkey", name: "Alice", trusted: true }),
      },
    });
    const { container } = renderUI(<DevicesPage />);
    const row = container.querySelector('[data-device-id="newkey"]');
    expect(row).not.toBeNull();
    fireEvent.click(row as Element);

    expect(useOverlays.getState().send).toBeNull();
    expect(useOverlays.getState().fpAlert).toEqual({
      deviceId: "oldkey",
      step: "warn",
    });
  });
});

describe("DevicesPage — manual IP connect", () => {
  it("calls api.connectByAddr then re-queries the device list", async () => {
    seedDevices([]);
    renderUI(<DevicesPage />);

    const input = screen.getByPlaceholderText("192.168.1.__");
    fireEvent.change(input, { target: { value: "192.168.1.42" } });
    fireEvent.click(screen.getByText(i18n.t("devices.ipDirect")));

    await waitFor(() =>
      expect(api.connectByAddr).toHaveBeenCalledWith("192.168.1.42"),
    );
    // it re-queries the merged list so a manually-added peer shows on the radar
    await waitFor(() => expect(api.listDiscoveredDevices).toHaveBeenCalled());
  });

  it("triggers the same flow on Enter in the IP field", async () => {
    seedDevices([]);
    renderUI(<DevicesPage />);

    const input = screen.getByPlaceholderText("192.168.1.__");
    fireEvent.change(input, { target: { value: "10.0.0.7:6000" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() =>
      expect(api.connectByAddr).toHaveBeenCalledWith("10.0.0.7:6000"),
    );
  });

  it("does not call connectByAddr for a blank address", () => {
    seedDevices([]);
    renderUI(<DevicesPage />);
    fireEvent.click(screen.getByText(i18n.t("devices.ipDirect")));
    expect(api.connectByAddr).not.toHaveBeenCalled();
  });
});
