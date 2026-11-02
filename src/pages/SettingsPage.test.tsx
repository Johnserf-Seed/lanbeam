// Component tests for SettingsPage. The env is non-Tauri, so the bridge setters
// take their browser-mode branch; we mock the specific setters we assert on
// (spreading the real module so isTauri / getNetworkInfo / demo data stay
// intact) and mock sendops.copyText so the fingerprint-copy is observable. Each
// test resets useData/usePrefs to a captured snapshot, then seeds only what it
// needs before rendering.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../bridge/api";
import type { MyIdentity, NetworkInfo, Settings } from "../bridge/api";
import i18n from "../i18n";
import { copyText } from "../lib/sendops";
import { useData, useOverlays, usePrefs, useToast } from "../lib/store";
import {
  checkForUpdate,
  openReleasePage,
  RELEASES_URL,
  type UpdateCheck,
} from "../lib/update";
import { act, fireEvent, renderUI, screen, waitFor } from "../test/render";
import SettingsPage from "./SettingsPage";

vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return {
    ...actual,
    // Deterministic + observable stand-ins for the setters under test.
    getNetworkInfo: vi.fn(() =>
      Promise.resolve([{ ip: "192.168.1.20", broadcast: null }]),
    ),
    setVerifyHash: vi.fn(() => Promise.resolve()),
    setTrayClose: vi.fn(() => Promise.resolve()),
    setStripExif: vi.fn(() => Promise.resolve()),
    setOrganize: vi.fn(() => Promise.resolve()),
    setListenPort: vi.fn(() => Promise.resolve()),
    setDownloadDir: vi.fn((p: string) => Promise.resolve(p)),
    // In Tauri this never resolves (the app restarts); a resolved stub is enough
    // to assert the call without driving the .catch toast path.
    resetIdentity: vi.fn(() => Promise.resolve()),
  };
});

vi.mock("../lib/sendops", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/sendops")>();
  return { ...actual, copyText: vi.fn().mockResolvedValue(true) };
});

// No test reaches GitHub: the check is answered by hand, and opening a page
// is only recorded.
vi.mock("../lib/update", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/update")>();
  return {
    ...actual,
    checkForUpdate: vi.fn(),
    openReleasePage: vi.fn(() => Promise.resolve()),
  };
});

// ── snapshots for reset ─────────────────────────────────────────────────────
const prefs0 = { ...usePrefs.getState() };
const data0 = { ...useData.getState() };
const overlays0 = { ...useOverlays.getState() };
const toast0 = { ...useToast.getState() };

const DEVICE_ID = "vJx0Qm8dR3keAbc1234";

function makeIdentity(): MyIdentity {
  return { deviceId: DEVICE_ID, shortId: "vJx0Qm8d", name: "书房" };
}

function makeSettings(over: Partial<Settings> = {}): Settings {
  return {
    deviceName: "书房",
    discoverable: true,
    autoOpenFolder: false,
    logLevel: "normal",
    recvPolicy: "trusted",
    port: 51704,
    trayClose: true,
    notifSystem: true,
    autostart: false,
    hotkeyEnabled: false,
    hotkey: "Alt+Space",
    verifyHash: true,
    conflict: "ask",
    organize: "device",
    maxConcurrent: 3,
    rateLimit: "unlimited",
    uiZoom: 1,
    clipShare: false,
    stripExif: true,
    ...over,
  };
}

const NET: NetworkInfo[] = [{ ip: "192.168.1.20", broadcast: null }];

function seed(settingsOver: Partial<Settings> = {}) {
  useData.setState({
    identity: makeIdentity(),
    settings: makeSettings(settingsOver),
    networkInfo: NET,
    downloadDir: "~/Downloads/LanBeam",
    listenPort: 51704,
  });
}

// Locate the Toggle button (a bare <button class="toggle">) that sits in the
// same .set-row as a given label text — Toggles carry no accessible name.
function toggleForLabel(label: string): HTMLButtonElement {
  const row = screen.getByText(label).closest(".set-row");
  const btn = row?.querySelector("button.toggle");
  if (!btn) throw new Error(`no toggle in row for "${label}"`);
  return btn as HTMLButtonElement;
}

beforeEach(() => {
  usePrefs.setState(prefs0, true);
  useData.setState(data0, true);
  useOverlays.setState(overlays0, true);
  useToast.setState(toast0, true);
  vi.clearAllMocks();
});

describe("SettingsPage rendering", () => {
  it("renders without throwing once identity/settings are seeded", () => {
    seed();
    renderUI(<SettingsPage />);
    // The section headers anchor each block; assert several are present.
    expect(screen.getByText(i18n.t("settings.secGeneral"))).toBeTruthy();
    expect(screen.getByText(i18n.t("settings.secRecv"))).toBeTruthy();
    expect(screen.getByText(i18n.t("settings.secBehavior"))).toBeTruthy();
    expect(screen.getByText(i18n.t("settings.secNetwork"))).toBeTruthy();
    expect(screen.getByText(i18n.t("settings.secPrivacy"))).toBeTruthy();
    expect(screen.getByText(i18n.t("settings.secAbout"))).toBeTruthy();
  });

  it("shows the device name and the download directory", () => {
    seed();
    renderUI(<SettingsPage />);
    expect(screen.getByText("书房")).toBeTruthy();
    expect(screen.getByText("~/Downloads/LanBeam")).toBeTruthy();
  });
});

describe("SettingsPage toggles → bridge setter + prefs mirror", () => {
  it("verifyHash toggle calls setVerifyHash(false) and flips the mirror", () => {
    seed();
    usePrefs.getState().set({ verifyHash: true });
    renderUI(<SettingsPage />);

    const toggle = toggleForLabel(i18n.t("settings.verify"));
    expect(toggle.className).toContain("on");

    fireEvent.click(toggle);

    expect(vi.mocked(api.setVerifyHash)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.setVerifyHash)).toHaveBeenCalledWith(false);
    expect(usePrefs.getState().verifyHash).toBe(false);
  });

  it("trayClose toggle calls setTrayClose(false) and flips the mirror", () => {
    seed();
    usePrefs.getState().set({ trayClose: true });
    renderUI(<SettingsPage />);

    fireEvent.click(toggleForLabel(i18n.t("settings.trayClose")));

    expect(vi.mocked(api.setTrayClose)).toHaveBeenCalledWith(false);
    expect(usePrefs.getState().trayClose).toBe(false);
  });

  it("stripExif toggle calls setStripExif(false) and flips the mirror", () => {
    seed();
    usePrefs.getState().set({ stripExif: true });
    renderUI(<SettingsPage />);

    fireEvent.click(toggleForLabel(i18n.t("settings.exif")));

    expect(vi.mocked(api.setStripExif)).toHaveBeenCalledWith(false);
    expect(usePrefs.getState().stripExif).toBe(false);
  });
});

describe("SettingsPage · 自动整理", () => {
  it("offers a folder per file type, and says sent folders stay whole (F7)", () => {
    seed();
    usePrefs.getState().set({ organize: "none" });
    renderUI(<SettingsPage />);
    const select = screen
      .getByText(i18n.t("settings.organizeType"))
      .closest("select") as HTMLSelectElement;

    fireEvent.change(select, { target: { value: "type" } });

    expect(vi.mocked(api.setOrganize)).toHaveBeenCalledWith("type");
    expect(usePrefs.getState().organize).toBe("type");
    expect(screen.getByText(i18n.t("settings.organizeTypeDesc"))).toBeTruthy();
  });

  // The backend ignores a mode it doesn't list, so an option missing there
  // would be picked, shown and saved here while files kept landing as before.
  it("offers exactly the modes the backend accepts", () => {
    const rs = readFileSync(
      resolve(process.cwd(), "src-tauri/src/settings.rs"),
      "utf8",
    );
    const list = rs.match(/ORGANIZE_MODES: \[&str; \d+\] = \[([^\]]*)\]/)?.[1];
    expect(list, "ORGANIZE_MODES not found — did settings.rs move?").toBeTruthy();
    const accepted = [...(list ?? "").matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    seed();
    renderUI(<SettingsPage />);
    const select = screen
      .getByText(i18n.t("settings.organizeType"))
      .closest("select") as HTMLSelectElement;
    const offered = [...select.options].map((o) => o.value);
    expect(offered.sort()).toEqual(accepted.sort());
  });
});

describe("SettingsPage fingerprint copy", () => {
  it("copies the deviceId and shows the copied toast", async () => {
    seed();
    renderUI(<SettingsPage />);

    fireEvent.click(screen.getByText(i18n.t("settings.copyFp")));

    expect(vi.mocked(copyText)).toHaveBeenCalledWith(DEVICE_ID);
    await waitFor(() =>
      expect(useToast.getState().msg).toBe(i18n.t("settings.fpCopied")),
    );
  });
});

describe("SettingsPage port field", () => {
  it("commits a changed listen port on blur via setListenPort", () => {
    seed({ port: 51704 });
    usePrefs.getState().set({ port: "51704" });
    renderUI(<SettingsPage />);

    const input = screen.getByDisplayValue("51704");
    fireEvent.change(input, { target: { value: "51888" } });
    fireEvent.blur(input);

    expect(vi.mocked(api.setListenPort)).toHaveBeenCalledWith(51888);
    expect(usePrefs.getState().port).toBe("51888");
  });

  it("rejects a privileged/out-of-range port and restores the effective value", () => {
    seed({ port: 51704 });
    usePrefs.getState().set({ port: "51704" });
    renderUI(<SettingsPage />);

    const input = screen.getByDisplayValue("51704");
    fireEvent.change(input, { target: { value: "80" } });
    fireEvent.blur(input);

    expect(vi.mocked(api.setListenPort)).not.toHaveBeenCalled();
    // canonicalized back to the effective default
    expect(usePrefs.getState().port).toBe("51704");
    expect(useToast.getState().msg).toBe(i18n.t("settings.portInvalid"));
  });
});

describe("SettingsPage download-dir button (browser mode)", () => {
  it("surfaces the milestone toast and does not call setDownloadDir", () => {
    seed();
    renderUI(<SettingsPage />);

    // Scope to the download-dir row — "Change…" is reused elsewhere.
    const row = screen
      .getByText(i18n.t("settings.downloadDir"))
      .closest(".set-row");
    const btn = row?.querySelector("button.btn");
    fireEvent.click(btn as HTMLButtonElement);

    expect(useToast.getState().msg).toBe(i18n.t("common.milestoneNote"));
    expect(vi.mocked(api.setDownloadDir)).not.toHaveBeenCalled();
  });
});

describe("SettingsPage · 检查更新 (F2)", () => {
  const checkButton = () =>
    screen.getByRole<HTMLButtonElement>("button", {
      name: i18n.t("settings.checkUpdate"),
    });

  it("asks nothing until pressed, then says a newer version is out and links to it", async () => {
    let answer: (r: UpdateCheck) => void = () => {};
    vi.mocked(checkForUpdate).mockReturnValue(
      new Promise((resolve) => {
        answer = resolve;
      }),
    );
    seed();
    renderUI(<SettingsPage />);
    expect(checkForUpdate).not.toHaveBeenCalled();
    expect(screen.getByText(i18n.t("settings.updateDesc"))).toBeTruthy();

    fireEvent.click(checkButton());
    const checking = screen.getByRole<HTMLButtonElement>("button", {
      name: i18n.t("settings.checkingUpdate"),
    });
    expect(checking.disabled).toBe(true);

    const url = "https://github.com/Johnserf-Seed/lanbeam/releases/tag/v0.1.4";
    await act(async () => answer({ kind: "newer", latest: "0.1.4", url }));
    expect(
      screen.getByText(i18n.t("settings.updateNewer", { latest: "0.1.4" }), {
        exact: false,
      }),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("settings.updateDownload") }),
    );
    expect(openReleasePage).toHaveBeenCalledWith(url);
  });

  it("says when this is the latest, and can be asked again", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue({
      kind: "current",
      latest: "0.1.3",
    });
    seed();
    renderUI(<SettingsPage />);
    fireEvent.click(checkButton());
    expect(
      await screen.findByText(i18n.t("settings.updateCurrent")),
    ).toBeTruthy();
    expect(checkButton().disabled).toBe(false);
  });

  it("says when it couldn't check, and offers the releases page instead", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue({ kind: "failed" });
    seed();
    renderUI(<SettingsPage />);
    fireEvent.click(checkButton());
    fireEvent.click(
      await screen.findByRole("button", {
        name: i18n.t("settings.updateOpenReleases"),
      }),
    );
    expect(openReleasePage).toHaveBeenCalledWith(RELEASES_URL);
    expect(
      screen.getByText(i18n.t("settings.updateFailed"), { exact: false }),
    ).toBeTruthy();
  });
});

describe("SettingsPage reset-identity two-step confirm", () => {
  it("arms on the first click and only resets on a later, deliberate second click", () => {
    vi.useFakeTimers();
    try {
      seed();
      renderUI(<SettingsPage />);

      const btn = screen.getByRole("button", {
        name: i18n.t("settings.resetBtn"),
      });

      // First click arms — the label switches, nothing is reset yet.
      fireEvent.click(btn);
      expect(btn.textContent).toBe(i18n.t("settings.resetConfirm"));
      expect(vi.mocked(api.resetIdentity)).not.toHaveBeenCalled();

      // Past the ~350ms double-click guard, the confirming click resets.
      vi.advanceTimersByTime(400);
      fireEvent.click(btn);
      expect(vi.mocked(api.resetIdentity)).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("swallows an accidental rapid double-click (stays armed, no reset)", () => {
    vi.useFakeTimers();
    try {
      seed();
      renderUI(<SettingsPage />);

      const btn = screen.getByRole("button", {
        name: i18n.t("settings.resetBtn"),
      });

      fireEvent.click(btn); // arm
      vi.advanceTimersByTime(100); // within the guard window
      fireEvent.click(btn); // swallowed

      expect(vi.mocked(api.resetIdentity)).not.toHaveBeenCalled();
      expect(btn.textContent).toBe(i18n.t("settings.resetConfirm"));
    } finally {
      vi.useRealTimers();
    }
  });
});
