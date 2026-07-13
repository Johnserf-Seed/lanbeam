// Component tests for Header — largely presentational: it renders a title +
// subtitle derived from the current route, action buttons on the devices page,
// and a theme-toggle. The env is non-Tauri so WindowControls renders nothing.
// Text is asserted via i18n.t(...) so we don't hard-code a resolved language.
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import {
  useData,
  useInbox,
  useOverlays,
  usePrefs,
  useRecents,
  useSysDark,
  useTransfers,
} from "../lib/store";
import { fireEvent, renderUI, screen } from "../test/render";
import Header from "./Header";

// ── snapshots for reset ─────────────────────────────────────────────────────
const prefs0 = { ...usePrefs.getState() };
const data0 = { ...useData.getState() };
const transfers0 = { ...useTransfers.getState() };
const inbox0 = { ...useInbox.getState() };
const recents0 = { ...useRecents.getState() };
const overlays0 = { ...useOverlays.getState() };
const sysDark0 = { ...useSysDark.getState() };

beforeEach(() => {
  usePrefs.setState(prefs0, true);
  useData.setState(data0, true);
  useTransfers.setState(transfers0, true);
  useInbox.setState(inbox0, true);
  useRecents.setState(recents0, true);
  useOverlays.setState(overlays0, true);
  useSysDark.setState(sysDark0, true);
});

describe("Header", () => {
  it("renders the devices title on the default route", () => {
    renderUI(<Header />);
    expect(screen.getByText(i18n.t("titles.devices"))).toBeTruthy();
  });

  it("shows the empty subtitle when no devices are present", () => {
    useData.setState({ devices: [] });
    renderUI(<Header />);
    expect(screen.getByText(i18n.t("titles.devicesSubEmpty"))).toBeTruthy();
  });

  it("shows the populated subtitle with device count + name", () => {
    useData.setState({
      devices: [
        { deviceId: "a", name: "A", address: "", port: 0 },
        { deviceId: "b", name: "B", address: "", port: 0 },
      ],
      settings: { deviceName: "MyBox" } as never,
    });
    renderUI(<Header />);
    const expected = i18n.t("titles.devicesSub", { n: 2, name: "MyBox" });
    expect(screen.getByText(expected)).toBeTruthy();
  });

  it("renders the pair / quick-text / send action buttons on the devices page", () => {
    renderUI(<Header />);
    expect(
      screen.getByRole("button", { name: i18n.t("header.pair") }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: i18n.t("header.quickText") }),
    ).toBeTruthy();
    expect(screen.getByText(i18n.t("header.sendFiles"))).toBeTruthy();
  });

  it("opens the pair overlay when the Pair button is clicked", () => {
    renderUI(<Header />);
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("header.pair") }),
    );
    expect(useOverlays.getState().pairOpen).toBe(true);
  });

  it("opens the quick-text overlay when the Quick text button is clicked", () => {
    renderUI(<Header />);
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("header.quickText") }),
    );
    expect(useOverlays.getState().qtOpen).toBe(true);
  });

  it("toggles the theme preference via the theme button", () => {
    usePrefs.setState({ themeMode: "light" });
    renderUI(<Header />);
    fireEvent.click(screen.getByTitle(i18n.t("header.toggleTheme")));
    expect(usePrefs.getState().themeMode).toBe("dark");
  });
});
