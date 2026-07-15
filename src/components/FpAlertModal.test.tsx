// Component tests for FpAlertModal — the warning that surfaces when a remembered
// device's name reappears under a different key. The env is non-Tauri, so trust
// writes take their browser-mode branch (no IPC).
//
// This file used to have a test called "migrates trust to the new key and closes
// when the codes match". It was pinning the dangerous version: the modal dialled
// the suspect key, showed the SAS that handshake produced, and told the user both
// screens should show it — while the other device displayed nothing at all. There
// was no second number to compare, so pressing 「一致」 handed trust to the very
// key the alert existed to make you suspicious of. Those tests are gone with the
// flow they guarded.
import { beforeEach, describe, expect, it } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import i18n from "../i18n";
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

function newKeyDevice(): DiscoveredDevice {
  return {
    deviceId: NEW_ID,
    name: NAME,
    address: "192.168.1.9",
  } as DiscoveredDevice;
}

// A remembered record whose name has reappeared under a new key, live on the LAN
// — so trustList() marks the old record fpChanged and the modal has something to
// report.
function seed() {
  useTrust.setState({ records: { [OLD_ID]: rec() }, sel: null });
  useData.setState({ devices: [newKeyDevice()], firstSeen: {} });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  useTrust.setState({ records: {}, sel: null });
  useData.setState({ devices: [], firstSeen: {} });
  useOverlays.setState({ fpAlert: null, pairOpen: false, pairPrefill: null });
  useToast.setState({ msg: null, action: null });
});

describe("FpAlertModal", () => {
  it("renders nothing when there is no fpAlert", () => {
    const { container } = renderUI(<FpAlertModal />);
    expect(container.querySelector(".scrim")).toBeNull();
  });

  it("shows the peer name and both keys, side by side", () => {
    seed();
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID } });
    renderUI(<FpAlertModal />);

    expect(
      screen.getByText(i18n.t("fp.warnTitle", { name: NAME })),
    ).toBeTruthy();
    expect(screen.getByText(i18n.t("fp.before"))).toBeTruthy();
    expect(screen.getByText(i18n.t("fp.now"))).toBeTruthy();
    // The evidence: the key you remembered, and the key that's here now.
    expect(screen.getByText("OLD1 OLD2 OLD3 OLD4")).toBeTruthy();
    expect(screen.getByText("NEW1 NEW2 NEW3 NEW4")).toBeTruthy();
  });

  it("offers NO way to trust the new key from here", () => {
    // The security contract, asserted. Trusting a key nobody has verified is
    // exactly what this alert exists to prevent — so the only paths out are
    // "delete the old record" and "go and pair, properly, with both screens".
    seed();
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID } });
    renderUI(<FpAlertModal />);

    // The only two buttons. Neither one grants trust.
    expect(screen.getByText(i18n.t("fp.forgetOld"))).toBeTruthy();
    expect(screen.getByText(i18n.t("fp.pairAgain"))).toBeTruthy();

    fireEvent.click(screen.getByText(i18n.t("fp.forgetOld")));
    expect(useTrust.getState().records[NEW_ID]).toBeUndefined();
  });

  it("deletes the old record on 删除旧记录, and says the new one still isn't trusted", () => {
    seed();
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID } });
    renderUI(<FpAlertModal />);

    fireEvent.click(screen.getByText(i18n.t("fp.forgetOld")));

    expect(useTrust.getState().records[OLD_ID]).toBeUndefined();
    expect(useOverlays.getState().fpAlert).toBeNull();
    expect(useToast.getState().msg).toBe(i18n.t("fp.removedToast"));
  });

  it("sends you to pairing, pre-filled with the new device's address", () => {
    // Pairing is the only honest way back: it shows the same code on BOTH
    // screens and records trust only once a person confirms they match.
    seed();
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID } });
    renderUI(<FpAlertModal />);

    fireEvent.click(screen.getByText(i18n.t("fp.pairAgain")));

    expect(useOverlays.getState().pairOpen).toBe(true);
    expect(useOverlays.getState().pairPrefill).toBe("192.168.1.9");
    expect(useOverlays.getState().fpAlert).toBeNull();
    // The old record is untouched — you may still want it, and deleting it is a
    // separate, explicit choice.
    expect(useTrust.getState().records[OLD_ID]).toBeTruthy();
  });

  it("clears a stale alert whose record or new key is gone", () => {
    useOverlays.setState({ fpAlert: { deviceId: OLD_ID } });
    const { container } = renderUI(<FpAlertModal />);
    expect(container.querySelector(".scrim")).toBeNull();
    expect(useOverlays.getState().fpAlert).toBeNull();
  });
});
