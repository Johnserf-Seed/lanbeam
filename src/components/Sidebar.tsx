import {
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import {
  useData,
  useInbox,
  usePrefs,
  useTransfers,
  useTrust,
  displayIp,
  setVisibility,
  showToast,
  trustList,
  visibilityOf,
  type Visibility,
} from "../lib/store";

/* CSS-drawn nav icons from the prototype (knockouts use the sidebar bg). */

function IconDevices({ color }: { color: string }) {
  return (
    <div
      style={{
        width: 17,
        height: 17,
        flex: "none",
        position: "relative",
        color,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          border: "1.5px solid currentColor",
          borderRadius: "50%",
          boxSizing: "border-box",
          opacity: 0.55,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: "50%",
          top: "50%",
          width: 5,
          height: 5,
          margin: "-2.5px 0 0 -2.5px",
          borderRadius: "50%",
          background: "currentColor",
        }}
      />
      <div
        style={{
          position: "absolute",
          right: 1,
          top: 1,
          width: 3.5,
          height: 3.5,
          borderRadius: "50%",
          background: "currentColor",
        }}
      />
    </div>
  );
}

function IconTransfers({ color }: { color: string }) {
  return (
    <div
      style={{
        width: 17,
        height: 17,
        flex: "none",
        position: "relative",
        color,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 1.5,
          border: "1.5px solid currentColor",
          borderRadius: "50%",
          boxSizing: "border-box",
          opacity: 0.9,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 1,
          top: 1,
          width: 5,
          height: 5,
          borderRadius: "50%",
          background: "var(--sidebar)",
        }}
      />
      <div
        style={{
          position: "absolute",
          right: 1,
          bottom: 1,
          width: 5,
          height: 5,
          borderRadius: "50%",
          background: "var(--sidebar)",
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 2.5,
          top: 1,
          width: 0,
          height: 0,
          borderLeft: "2.5px solid transparent",
          borderRight: "2.5px solid transparent",
          borderBottom: "4.5px solid currentColor",
        }}
      />
      <div
        style={{
          position: "absolute",
          right: 2.5,
          bottom: 1,
          width: 0,
          height: 0,
          borderLeft: "2.5px solid transparent",
          borderRight: "2.5px solid transparent",
          borderTop: "4.5px solid currentColor",
        }}
      />
    </div>
  );
}

function IconInbox({ color }: { color: string }) {
  return (
    <div
      style={{
        width: 17,
        height: 17,
        flex: "none",
        position: "relative",
        color,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 1.5,
          border: "1.5px solid currentColor",
          borderRadius: "50%",
          boxSizing: "border-box",
          opacity: 0.9,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: "50%",
          top: 0,
          width: 6,
          height: 5.5,
          marginLeft: -3,
          background: "var(--sidebar)",
        }}
      />
      <div
        style={{
          position: "absolute",
          left: "50%",
          top: 0,
          width: 2,
          height: 6,
          marginLeft: -1,
          background: "currentColor",
          borderRadius: 1,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: "50%",
          top: 5.5,
          marginLeft: -3,
          width: 0,
          height: 0,
          borderLeft: "3px solid transparent",
          borderRight: "3px solid transparent",
          borderTop: "4px solid currentColor",
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 5.5,
          right: 5.5,
          bottom: 3.5,
          height: 1.5,
          background: "currentColor",
          borderRadius: 1,
        }}
      />
    </div>
  );
}

function IconTrusted({ color }: { color: string }) {
  return (
    <div
      style={{
        width: 17,
        height: 17,
        flex: "none",
        position: "relative",
        color,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0.5,
          border: "1.5px dashed currentColor",
          borderRadius: "50%",
          boxSizing: "border-box",
          opacity: 0.8,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: "50%",
          top: "50%",
          width: 5,
          height: 5,
          margin: "-2.5px 0 0 -2.5px",
          borderRadius: "50%",
          background: "currentColor",
        }}
      />
    </div>
  );
}

function IconSettings({ color }: { color: string }) {
  const line: CSSProperties = {
    width: 15,
    height: 1.5,
    borderRadius: 1,
    background: "currentColor",
  };
  return (
    <div
      style={{
        width: 17,
        height: 17,
        flex: "none",
        display: "flex",
        flexDirection: "column",
        justifyContent: "center",
        gap: 3.5,
        color,
      }}
    >
      <div style={line} />
      <div style={{ ...line, position: "relative" }}>
        <div
          style={{
            position: "absolute",
            right: 2.5,
            top: -2.25,
            width: 6,
            height: 6,
            borderRadius: "50%",
            background: "var(--sidebar)",
            border: "1.5px solid currentColor",
            boxSizing: "border-box",
          }}
        />
      </div>
      <div style={line} />
    </div>
  );
}

function NavItem({
  to,
  icon,
  label,
  count,
  countAccent,
}: {
  to: string;
  icon: (color: string) => ReactNode;
  label: string;
  count?: string;
  countAccent?: boolean;
}) {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const on = pathname === to;
  const c = on ? "var(--accent-ink)" : "var(--muted2)";
  return (
    // biome-ignore lint/a11y/useSemanticElements: styled nav row kept — a <button> would change the row's layout/styling; made keyboard-operable via role/tabIndex/onKeyDown
    <div
      onClick={() => navigate(to)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          if (e.key === " ") e.preventDefault();
          navigate(to);
        }
      }}
      role="button"
      tabIndex={0}
      className="hover-row"
      style={{
        position: "relative",
        display: "flex",
        alignItems: "center",
        gap: 9,
        padding: "7px 10px",
        borderRadius: 7,
        cursor: "pointer",
      }}
    >
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 6,
          bottom: 6,
          width: 2.5,
          borderRadius: 3,
          background: on ? "var(--accent)" : "transparent",
        }}
      />
      {icon(c)}
      <span style={{ fontSize: 12.5, fontWeight: on ? 650 : 500, color: c }}>
        {label}
      </span>
      {count ? (
        <span
          className="mono"
          style={{
            marginLeft: "auto",
            fontSize: 10,
            fontWeight: countAccent ? 600 : 400,
            color: countAccent ? "var(--accent-ink)" : "var(--muted)",
          }}
        >
          {count}
        </span>
      ) : null}
    </div>
  );
}

const VIS_META: Record<Visibility, { dot: string; check: string }> = {
  on: { dot: "var(--success)", check: "var(--success)" },
  ghost: { dot: "var(--accent)", check: "var(--accent-ink)" },
  off: { dot: "var(--muted)", check: "var(--muted2)" },
};

export default function Sidebar() {
  const { t } = useTranslation();
  const identity = useData((s) => s.identity);
  const settings = useData((s) => s.settings);
  const devices = useData((s) => s.devices);
  // Local IPv4 (M5.1) — the slot next to the shortId the design mock reserved
  // for the real address. Prefer the interface the user pinned (M5.6) over the
  // numerically-first entry, which is often a VPN/virtual adapter.
  const networkInfo = useData((s) => s.networkInfo);
  const iface = usePrefs((s) => s.iface);
  const localIp = displayIp(networkInfo, iface);
  const ghostUntil = usePrefs((s) => s.ghostUntil);
  // Select the badge count directly so zustand's Object.is check suppresses
  // Sidebar re-renders during steady-state progress: the ~10-20 progress writes
  // per second give s.transfers a new identity every tick, but the derived count
  // only changes when a transfer starts or ends. "queued" (parked on the
  // concurrency gate) is in-progress too, so it counts alongside "active".
  const running = useTransfers((s) => {
    let n = 0;
    for (const k in s.transfers) {
      const st = s.transfers[k].status;
      if (st === "active" || st === "queued") n++;
    }
    return n;
  });
  const unread = useInbox((s) => s.unread);
  const records = useTrust((s) => s.records);
  const [menuOpen, setMenuOpen] = useState(false);
  const [, tick] = useState(0);

  // Keep the ghost countdown fresh.
  useEffect(() => {
    if (!ghostUntil) return;
    const id = setInterval(() => tick((v) => v + 1), 30_000);
    return () => clearInterval(id);
  }, [ghostUntil]);

  // Re-enumerate interfaces whenever the window regains focus (M5.6): with
  // close-to-tray the app lives for days, so a DHCP renumber / Wi-Fi switch /
  // NIC hotplug while hidden must not leave the sidebar IP stale until restart.
  useEffect(() => {
    if (!api.isTauri) return;
    let un: (() => void) | undefined;
    let disposed = false;
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      if (disposed) return;
      void getCurrentWindow()
        .onFocusChanged(({ payload: focused }) => {
          if (focused) void useData.getState().refreshNetworkInfo();
        })
        .then((off) => {
          if (disposed) off();
          else un = off;
        });
    });
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  const vis = visibilityOf(settings, ghostUntil);
  const tl = useMemo(() => trustList(devices, records), [devices, records]);
  const trustedCount = tl.filter((d) => d.trusted).length;

  const deviceName = settings?.deviceName || identity?.name || "";
  const ghostMins = ghostUntil
    ? Math.max(1, Math.ceil((ghostUntil - Date.now()) / 60000))
    : 0;
  const stateText =
    vis === "on"
      ? t("vis.onState")
      : vis === "ghost"
        ? t("vis.ghostState", { mins: ghostMins })
        : t("vis.offState");
  const stateColor =
    vis === "on"
      ? "var(--success)"
      : vis === "ghost"
        ? "var(--accent-ink)"
        : "var(--muted)";

  const pick = async (v: Visibility) => {
    setMenuOpen(false);
    await setVisibility(v);
    if (v === "ghost") showToast(t("vis.ghostToast"));
  };

  const menuRow = (v: Visibility) => (
    // biome-ignore lint/a11y/useSemanticElements: styled visibility-option row kept — a <button> would change the custom layout/styling; made keyboard-operable via role/tabIndex/onKeyDown
    <div
      key={v}
      onClick={() => pick(v)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          if (e.key === " ") e.preventDefault();
          pick(v);
        }
      }}
      role="button"
      tabIndex={0}
      className="hover-row"
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 8,
        padding: "7px 9px",
        borderRadius: 8,
        background:
          vis === v
            ? v === "on"
              ? "var(--success-soft)"
              : v === "ghost"
                ? "var(--accent-soft)"
                : "var(--track)"
            : "transparent",
        cursor: "pointer",
      }}
    >
      <span
        style={{
          width: 7,
          height: 7,
          borderRadius: "50%",
          background: VIS_META[v].dot,
          flex: "none",
          marginTop: 3.5,
        }}
      />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 11, fontWeight: 600, color: "var(--ink2)" }}>
          {t(`vis.${v}`)}
        </div>
        <div style={{ fontSize: 9.5, color: "var(--muted)", marginTop: 1 }}>
          {t(`vis.${v}Desc`)}
        </div>
      </div>
      <span
        style={{
          fontSize: 10.5,
          fontWeight: 600,
          color: VIS_META[v].check,
          flex: "none",
        }}
      >
        {vis === v ? "✓" : ""}
      </span>
    </div>
  );

  return (
    <>
      <aside
        style={{
          width: 214,
          flex: "none",
          background: "var(--sidebar)",
          borderRight: "1px solid var(--border)",
          display: "flex",
          flexDirection: "column",
          padding: "16px 12px 14px",
          transition: "background .25s ease",
        }}
      >
        <div
          data-tauri-drag-region=""
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "6px 8px 20px",
          }}
        >
          <div
            data-tauri-drag-region=""
            style={{
              width: 30,
              height: 30,
              borderRadius: 9,
              background: "var(--accent)",
              color: "var(--accent-fg)",
              display: "grid",
              placeItems: "center",
              fontSize: 15,
              fontWeight: 600,
              flex: "none",
            }}
          >
            ⌁
          </div>
          <div data-tauri-drag-region="">
            <div
              data-tauri-drag-region=""
              style={{
                fontWeight: 700,
                fontSize: 14.5,
                letterSpacing: "-.01em",
                color: "var(--ink2)",
              }}
            >
              {t("app.name")}
            </div>
            <div
              data-tauri-drag-region=""
              style={{ fontSize: 10.5, color: "var(--muted)" }}
            >
              {t("app.tagline")}
            </div>
          </div>
        </div>

        <nav style={{ display: "flex", flexDirection: "column" }}>
          <div
            style={{
              fontSize: 9.5,
              fontWeight: 600,
              letterSpacing: ".08em",
              color: "var(--muted)",
              padding: "0 10px 5px",
            }}
          >
            {t("nav.sectionTransfer")}
          </div>
          <NavItem
            to="/"
            icon={(c) => <IconDevices color={c} />}
            label={t("nav.devices")}
            count={devices.length ? String(devices.length) : ""}
          />
          <NavItem
            to="/transfers"
            icon={(c) => <IconTransfers color={c} />}
            label={t("nav.transfers")}
            count={running ? String(running) : ""}
            countAccent
          />
          <NavItem
            to="/inbox"
            icon={(c) => <IconInbox color={c} />}
            label={t("nav.inbox")}
            count={unread ? String(unread) : ""}
            countAccent
          />
          <div
            style={{
              fontSize: 9.5,
              fontWeight: 600,
              letterSpacing: ".08em",
              color: "var(--muted)",
              padding: "14px 10px 5px",
            }}
          >
            {t("nav.sectionManage")}
          </div>
          <NavItem
            to="/trusted"
            icon={(c) => <IconTrusted color={c} />}
            label={t("nav.trusted")}
            count={tl.length ? `${trustedCount}/${tl.length}` : ""}
          />
          <NavItem
            to="/settings"
            icon={(c) => <IconSettings color={c} />}
            label={t("nav.settings")}
          />
        </nav>

        <div
          style={{
            marginTop: "auto",
            borderTop: "1px solid var(--border)",
            padding: "11px 8px 2px",
            position: "relative",
            zIndex: 41,
          }}
        >
          {menuOpen && (
            <div
              style={{
                position: "absolute",
                left: 0,
                right: 0,
                bottom: "calc(100% + 4px)",
                background: "var(--panel)",
                border: "1px solid var(--border2)",
                borderRadius: 11,
                boxShadow: "0 10px 26px rgba(0,0,0,.12)",
                padding: 5,
                animation: "lbFade .15s ease",
              }}
            >
              {(["on", "ghost", "off"] as Visibility[]).map(menuRow)}
            </div>
          )}
          {/* biome-ignore lint/a11y/useSemanticElements: styled visibility switch kept — a <button> would change the row's layout/styling; made keyboard-operable via role/tabIndex/onKeyDown */}
          <div
            onClick={() => setMenuOpen((v) => !v)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                if (e.key === " ") e.preventDefault();
                setMenuOpen((v) => !v);
              }
            }}
            role="button"
            tabIndex={0}
            title={t("vis.switchTitle")}
            className="hover-row"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 7,
              padding: "4px 6px",
              margin: "0 -6px",
              borderRadius: 7,
              background: menuOpen ? "var(--hover)" : "transparent",
              cursor: "pointer",
            }}
          >
            <span
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                background: VIS_META[vis].dot,
                flex: "none",
              }}
            />
            <span
              style={{
                fontSize: 11.5,
                fontWeight: 600,
                color: "var(--ink2)",
                flex: 1,
                minWidth: 0,
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {deviceName}
            </span>
            <span
              style={{ fontSize: 8.5, color: "var(--muted)", flex: "none" }}
            >
              {menuOpen ? "▴" : "▾"}
            </span>
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "baseline",
              gap: 6,
              marginTop: 2,
              paddingLeft: 15,
              whiteSpace: "nowrap",
              overflow: "hidden",
            }}
          >
            {identity && (
              <span
                className="mono"
                style={{ fontSize: 9.5, color: "var(--muted)" }}
              >
                {identity.shortId}
              </span>
            )}
            {localIp && (
              <span
                className="mono"
                style={{ fontSize: 9.5, color: "var(--muted)" }}
              >
                {localIp}
              </span>
            )}
            <span style={{ fontSize: 9.5, color: stateColor }}>
              {stateText}
            </span>
          </div>
        </div>
      </aside>
      {menuOpen && (
        // biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc
        // biome-ignore lint/a11y/useKeyWithClickEvents: same
        <div
          onClick={() => setMenuOpen(false)}
          style={{ position: "fixed", inset: 0, zIndex: 40 }}
        />
      )}
    </>
  );
}
