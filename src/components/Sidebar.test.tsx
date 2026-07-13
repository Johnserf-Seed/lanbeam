// Component tests for the sidebar nav shell. The env is non-Tauri, so the
// focus-listener effect (api.isTauri) is skipped and the bridge is never hit —
// these cases only exercise pure rendering off seeded zustand state. Stores are
// reset to a captured snapshot before each test so cases can't leak.
import { beforeEach, describe, expect, it } from "vitest";
import type { DiscoveredDevice, MyIdentity, Settings } from "../bridge/api";
import i18n from "../i18n";
import {
  useData,
  useInbox,
  usePrefs,
  useTransfers,
  useTrust,
} from "../lib/store";
import type { UITransfer } from "../lib/store";
import { renderUI, screen, within } from "../test/render";
import Sidebar from "./Sidebar";

// ── snapshots for reset ────────────────────────────────────────────────────
const data0 = { ...useData.getState() };
const prefs0 = { ...usePrefs.getState() };
const transfers0 = { ...useTransfers.getState() };
const inbox0 = { ...useInbox.getState() };
const trust0 = { ...useTrust.getState() };

beforeEach(() => {
  useData.setState(data0, true);
  usePrefs.setState(prefs0, true);
  useTransfers.setState(transfers0, true);
  useInbox.setState(inbox0, true);
  useTrust.setState(trust0, true);
});

const t = (key: string, opts?: Record<string, unknown>) => i18n.t(key, opts);

const device = (id: string): DiscoveredDevice => ({
  deviceId: id,
  name: id,
  address: "1.1.1.1",
  port: 1,
});

const transfer = (
  sessionId: string,
  status: UITransfer["status"],
): UITransfer =>
  ({
    sessionId,
    direction: "send",
    status,
    percent: 0,
    totalSize: 0,
    speedBps: 0,
    hist: [],
    startedAt: 0,
  }) as UITransfer;

describe("Sidebar", () => {
  it("renders the app name and every nav item", () => {
    renderUI(<Sidebar />);
    expect(screen.getByText(t("app.name"))).toBeInTheDocument();
    // section headers
    expect(screen.getByText(t("nav.sectionTransfer"))).toBeInTheDocument();
    expect(screen.getByText(t("nav.sectionManage"))).toBeInTheDocument();
    // nav labels
    expect(screen.getByText(t("nav.devices"))).toBeInTheDocument();
    expect(screen.getByText(t("nav.transfers"))).toBeInTheDocument();
    expect(screen.getByText(t("nav.inbox"))).toBeInTheDocument();
    expect(screen.getByText(t("nav.trusted"))).toBeInTheDocument();
    expect(screen.getByText(t("nav.settings"))).toBeInTheDocument();
  });

  it("marks the default '/' route (Devices) active and others inactive", () => {
    // renderUI mounts a MemoryRouter at "/", so the Devices item is on.
    renderUI(<Sidebar />);
    const active = screen.getByText(t("nav.devices"));
    const inactive = screen.getByText(t("nav.settings"));
    // NavItem renders the active label at fontWeight 650, inactive at 500.
    expect(active.style.fontWeight).toBe("650");
    expect(inactive.style.fontWeight).toBe("500");
  });

  it("shows the device count badge from useData.devices", () => {
    useData.setState({ devices: [device("a"), device("b"), device("c")] });
    renderUI(<Sidebar />);
    const row = screen.getByText(t("nav.devices")).parentElement as HTMLElement;
    expect(within(row).getByText("3")).toBeInTheDocument();
  });

  it("counts only active + queued transfers in the badge (perf-fix selector)", () => {
    // active + queued are in-progress; done + error must NOT be counted.
    useData.setState({ devices: [] });
    useTransfers.setState({
      transfers: {
        s1: transfer("s1", "active"),
        s2: transfer("s2", "queued"),
        s3: transfer("s3", "done"),
        s4: transfer("s4", "error"),
      },
    });
    renderUI(<Sidebar />);
    const row = screen.getByText(t("nav.transfers"))
      .parentElement as HTMLElement;
    // 1 active + 1 queued = 2 (not 4)
    expect(within(row).getByText("2")).toBeInTheDocument();
    expect(within(row).queryByText("4")).not.toBeInTheDocument();
  });

  it("renders no transfers badge when nothing is in progress", () => {
    useTransfers.setState({
      transfers: { s3: transfer("s3", "done") },
    });
    renderUI(<Sidebar />);
    const row = screen.getByText(t("nav.transfers"))
      .parentElement as HTMLElement;
    // count is "" → the badge span isn't rendered; only the label text remains.
    expect(within(row).queryByText("1")).not.toBeInTheDocument();
    expect(within(row).getByText(t("nav.transfers"))).toBeInTheDocument();
  });

  it("shows the unread inbox badge", () => {
    useData.setState({ devices: [] });
    useInbox.setState({ unread: 5 });
    renderUI(<Sidebar />);
    const row = screen.getByText(t("nav.inbox")).parentElement as HTMLElement;
    expect(within(row).getByText("5")).toBeInTheDocument();
  });

  it("renders the identity shortId, local IP, and device name", () => {
    const identity: MyIdentity = {
      deviceId: "vJx0Qm8dR3keSomeLongPublicKeyValue0000000000",
      shortId: "vJx0Qm8d",
      name: "My Mac",
    };
    useData.setState({
      identity,
      settings: { deviceName: "My Mac" } as Settings,
      networkInfo: [{ ip: "192.168.1.42", broadcast: null }],
    });
    usePrefs.setState({ ...usePrefs.getState(), iface: "" });
    renderUI(<Sidebar />);
    // shortId (mono) in the identity strip
    expect(screen.getByText("vJx0Qm8d")).toBeInTheDocument();
    // local IPv4 chosen by displayIp (first entry when no iface filter)
    expect(screen.getByText("192.168.1.42")).toBeInTheDocument();
    // device name in the visibility switch row
    expect(screen.getByText("My Mac")).toBeInTheDocument();
    // visibility state text: settings.discoverable falsy + no ghost → "off"
    expect(screen.getByText(t("vis.offState"))).toBeInTheDocument();
  });

  it("falls back to identity.name when settings has no deviceName", () => {
    useData.setState({
      identity: {
        deviceId: "abc",
        shortId: "abc12345",
        name: "Fallback Name",
      } as MyIdentity,
      settings: null,
    });
    renderUI(<Sidebar />);
    expect(screen.getByText("Fallback Name")).toBeInTheDocument();
  });
});
