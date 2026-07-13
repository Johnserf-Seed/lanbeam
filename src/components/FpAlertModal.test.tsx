// Component tests for FpAlertModal — the fingerprint-changed warning that
// surfaces when a remembered device's name reappears under a different key. The
// env is non-Tauri, so trust writes take their browser-mode branch (no IPC).
import { beforeEach, describe, expect, it } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import i18n from "../i18n";
import { fmtSas } from "../lib/format";
import type { TrustRecord } from "../lib/store";
import { useData, useOverlays, useToast, useTrust } from "../lib/store";
import { fireEvent, renderUI, screen } from "../test/render";
import FpAlertModal from "./FpAlertModal";

const OLD_ID = "OLD1OLD2OLD3OLD4";
const NEW_ID = "NEW1NEW2NEW3NEW4";
const NAME = "客厅 · Mac mini";

function rec(over: Partial<TrustRecord> = {}): TrustRecord {
  return {
    deviceId: OLD_ID,
    name: NAME,
    trusted: true,
    autoAccept: true,
    addedAt: 1000,
    lastSeen: 2000,
    ...over,
  };
}

function imposterDevice(): DiscoveredDevice {
  return {
    deviceId: NEW_ID,
    name: NAME,
    address: "192.168.1.9",
  } as DiscoveredDevice;
}

// Seed a record whose name reappeared under a new key (imposter live device),
// so trustList() marks the old record fpChanged → the modal has an imposterId.
function seed() {
  useTrust.setState({ records: { [OLD_ID]: rec() }, sel: null });
  useData.setState({ devices: [imposterDevice()], firstSeen: {} });
}

beforeEach(async () => {
  // Deterministic language so i18n.t() lookups match rendered text.
  await i18n.changeLanguage("en");
  useTrust.setState({ records: {}, sel: null });
  useData.setState({ devices: [], firstSeen: {} });
  useOverlays.setState({ fpAlert: null });
  useToast.setState({ msg: null, action: null });
});

describe("FpAlertModal", () => {
  it("renders nothing when there is no fpAlert", () => {
    const { container } = renderUI(<FpAlertModal />);
    expect(container.querySelector(".scrim")).toBeNull();
  });

  it("renders the warn step with peer name and old/new fingerprints", () => {
    seed();
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID, step: "warn" } });
    renderUI(<FpAlertModal />);

    // Title carries the peer name.
    expect(
      screen.getByText(i18n.t("fp.warnTitle", { name: NAME })),
    ).toBeTruthy();
    // Before / Now labels.
    expect(screen.getByText(i18n.t("fp.before"))).toBeTruthy();
    expect(screen.getByText(i18n.t("fp.now"))).toBeTruthy();
    // Both fingerprints render as 4 groups of 4 (uppercased alnum of the ids).
    expect(screen.getByText("OLD1 OLD2 OLD3 OLD4")).toBeTruthy();
    expect(screen.getByText("NEW1 NEW2 NEW3 NEW4")).toBeTruthy();
  });

  it("removes trust and closes when the user picks Remove trust", () => {
    seed();
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID, step: "warn" } });
    renderUI(<FpAlertModal />);

    fireEvent.click(screen.getByText(i18n.t("fp.removeTrust")));

    expect(useTrust.getState().records[OLD_ID]).toBeUndefined();
    expect(useOverlays.getState().fpAlert).toBeNull();
    expect(useToast.getState().msg).toBe(i18n.t("fp.removedToast"));
  });

  it("renders the verify step with the SAS derived from the new key", () => {
    seed();
    const sas = "483921067";
    useOverlays.setState({
      fpAlert: { deviceId: OLD_ID, step: "verify", sas },
    });
    renderUI(<FpAlertModal />);

    expect(
      screen.getByText(i18n.t("fp.verifyTitle", { name: NAME })),
    ).toBeTruthy();
    expect(screen.getByText(i18n.t("fp.sasLabel"))).toBeTruthy();
    expect(screen.getByText(fmtSas(sas))).toBeTruthy();
  });

  it("migrates trust to the new key and closes when the codes match", () => {
    seed();
    useOverlays.setState({
      fpAlert: { deviceId: OLD_ID, step: "verify", sas: "483921067" },
    });
    renderUI(<FpAlertModal />);

    fireEvent.click(screen.getByText(i18n.t("fp.match")));

    const records = useTrust.getState().records;
    expect(records[OLD_ID]).toBeUndefined();
    const migrated = records[NEW_ID];
    expect(migrated).toBeTruthy();
    expect(migrated.trusted).toBe(true);
    expect(migrated.name).toBe(NAME);
    expect(useOverlays.getState().fpAlert).toBeNull();
    expect(useToast.getState().msg).toBe(i18n.t("fp.matchToast"));
  });

  it("removes trust and closes when the codes are Different", () => {
    seed();
    useOverlays.setState({
      fpAlert: { deviceId: OLD_ID, step: "verify", sas: "483921067" },
    });
    renderUI(<FpAlertModal />);

    fireEvent.click(screen.getByText(i18n.t("fp.mismatch")));

    expect(useTrust.getState().records[OLD_ID]).toBeUndefined();
    expect(useTrust.getState().records[NEW_ID]).toBeUndefined();
    expect(useOverlays.getState().fpAlert).toBeNull();
    expect(useToast.getState().msg).toBe(i18n.t("fp.mismatchToast"));
  });

  it("clears a stale alert whose record no longer exists", () => {
    // fpAlert points at a device with no record / no imposter → stale effect.
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID, step: "warn" } });
    const { container } = renderUI(<FpAlertModal />);
    expect(container.querySelector(".scrim")).toBeNull();
    expect(useOverlays.getState().fpAlert).toBeNull();
  });
});
