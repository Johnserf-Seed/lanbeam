import { useMemo } from "react";
import { useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import WindowControls from "./WindowControls";
import {
  useData,
  useInbox,
  useOverlays,
  usePrefs,
  useRecents,
  useSysDark,
  useTransfers,
  resolvedTheme,
  showToast,
  transferList,
} from "../lib/store";

export default function Header() {
  const { t } = useTranslation();
  const { pathname } = useLocation();
  const themeMode = usePrefs((s) => s.themeMode);
  const sysDark = useSysDark((s) => s.dark);
  const setPrefs = usePrefs((s) => s.set);
  const devices = useData((s) => s.devices);
  const settings = useData((s) => s.settings);
  const identity = useData((s) => s.identity);
  const transfers = useTransfers((s) => s.transfers);
  const inboxCount = useInbox((s) => s.items.length);
  const openSend = useOverlays((s) => s.openSend);
  const setPair = useOverlays((s) => s.setPair);
  const setQt = useOverlays((s) => s.setQt);
  const recents = useRecents((s) => s.items);

  const theme = resolvedTheme(themeMode, sysDark);
  const deviceName = settings?.deviceName || identity?.name || "";

  const { running, history } = useMemo(() => {
    const list = transferList(transfers);
    // "queued" (parked on the concurrency gate) is in-progress, so it counts as
    // running here too — mirroring TransfersPage's running/history split.
    return {
      running: list.filter(
        (x) => x.status === "active" || x.status === "queued",
      ).length,
      history: list.filter((x) => x.status === "done" || x.status === "error")
        .length,
    };
  }, [transfers]);

  const isDevices = pathname === "/";
  let title = t("titles.devices");
  let sub = devices.length
    ? t("titles.devicesSub", { n: devices.length, name: deviceName })
    : t("titles.devicesSubEmpty");
  if (pathname === "/transfers") {
    title = t("titles.transfers");
    sub = t("titles.transfersSub", { running, history });
  } else if (pathname === "/inbox") {
    title = t("titles.inbox");
    sub = t("titles.inboxSub", { n: inboxCount });
  } else if (pathname === "/trusted") {
    title = t("titles.trusted");
    sub = t("titles.trustedSub");
  } else if (pathname === "/settings") {
    title = t("titles.settings");
    sub = t("titles.settingsSub");
  }

  const sendGlobal = () => {
    if (!devices.length) {
      showToast(t("devices.noDevicesToast"));
      return;
    }
    openSend(null, recents);
  };

  return (
    <header
      data-tauri-drag-region=""
      style={{
        height: 62,
        flex: "none",
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "0 14px 0 24px",
        borderBottom: "1px solid var(--border)",
      }}
    >
      <div data-tauri-drag-region="">
        <div
          data-tauri-drag-region=""
          style={{
            fontSize: 15.5,
            fontWeight: 650,
            letterSpacing: "-.01em",
            color: "var(--ink2)",
          }}
        >
          {title}
        </div>
        <div
          data-tauri-drag-region=""
          style={{ fontSize: 11, color: "var(--muted)" }}
        >
          {sub}
        </div>
      </div>
      <div
        data-tauri-drag-region=""
        style={{ flex: 1, alignSelf: "stretch" }}
      />
      {isDevices && (
        <>
          <button
            type="button"
            className="btn"
            title={t("header.pairTitle")}
            onClick={() => setPair(true)}
          >
            {t("header.pair")}
          </button>
          <button
            type="button"
            className="btn"
            title={t("header.quickTextTitle")}
            onClick={() => setQt(true)}
          >
            {t("header.quickText")}
          </button>
          <button type="button" className="btn primary" onClick={sendGlobal}>
            <span style={{ fontSize: 14, lineHeight: 1 }}>↑</span>
            {t("header.sendFiles")}
          </button>
        </>
      )}
      <button
        type="button"
        className="btn icon"
        title={t("header.toggleTheme")}
        onClick={() =>
          setPrefs({ themeMode: theme === "light" ? "dark" : "light" })
        }
      >
        {theme === "light" ? "☾" : "☀"}
      </button>
      <WindowControls />
    </header>
  );
}
