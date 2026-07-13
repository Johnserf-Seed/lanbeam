import { useEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import {
  useData,
  usePrefs,
  useOverlays,
  showToast,
  displayIp,
  visibilityOf,
  setVisibility,
} from "../lib/store";
import type { Visibility, ThemeMode } from "../lib/store";
import { Toggle, Segmented } from "../components/ui";
import { copyText, errText, openDir, revealFile } from "../lib/sendops";
import { playSound } from "../lib/sound";
import type { SoundKind } from "../lib/sound";

/* fingerprint display: 4 groups of 4 from the deviceId's alphanumerics */
function fpGroups(deviceId: string): string {
  const hex = deviceId.replace(/[^a-zA-Z0-9]/g, "").toUpperCase();
  return [hex.slice(0, 4), hex.slice(4, 8), hex.slice(8, 12), hex.slice(12, 16)]
    .filter(Boolean)
    .join(" · ");
}

function isMac(): boolean {
  const ua =
    (typeof navigator !== "undefined" &&
      (navigator.platform || navigator.userAgent)) ||
    "";
  return /Mac/i.test(ua);
}

/* Platform-format a canonical accelerator ("Alt+Space", "Ctrl+Shift+K") for
   display. On macOS each modifier becomes its glyph (⌃⌥⇧⌘) laid out adjacent and
   space-joined ("⌥ Space"); elsewhere they stay spelled out and " + "-joined
   ("Alt + Space"). The STORED value is always the canonical accelerator — the
   backend parses that form (tauri global-shortcut syntax), and rebinding writes
   it back canonical; this only touches display. */
function formatHotkey(accel: string): string {
  const mac = isMac();
  const mods: Record<string, string> = mac
    ? { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Super: "⌘" }
    : { Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift", Super: "Win" };
  const parts = accel
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  return parts.map((p) => mods[p] ?? p).join(mac ? " " : " + ");
}

/* KeyboardEvent.code values for the modifier keys themselves — while only these
   are held the capture shows a preview but does not commit (no main key yet). */
const MOD_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
]);

/* The main-key token for an accelerator from a KeyboardEvent.code, or null for a
   code the tauri global-shortcut parser can't bind. Letters/digits collapse to
   "K"/"5"; the rest pass the code through verbatim (the parser matches it
   case-insensitively: "Space", "F5", "ArrowUp", "Semicolon", …). */
function keyToken(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  if (
    /^(Space|Tab|Enter|Backspace|Delete|Home|End|PageUp|PageDown|Insert|PrintScreen|Arrow(Up|Down|Left|Right)|F([1-9]|1[0-2])|Numpad[0-9]|Semicolon|Quote|Comma|Period|Slash|Backslash|Minus|Equal|BracketLeft|BracketRight|Backquote)$/.test(
      code,
    )
  ) {
    return code;
  }
  return null;
}

/* The modifier tokens currently held, in the canonical accelerator order
   (Ctrl → Alt → Shift → Super) so a rebind reads back consistently. */
function heldMods(e: {
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}): string[] {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  return mods;
}

const SOUNDS: { v: SoundKind; k: string }[] = [
  { v: "叮咚", k: "settings.soundDing" },
  { v: "清脆叮", k: "settings.soundCrisp" },
  { v: "水滴", k: "settings.soundDrop" },
  { v: "木鱼", k: "settings.soundWood" },
];

const stepBtn: CSSProperties = {
  width: 24,
  height: 24,
  borderRadius: 7,
  border: "1px solid var(--border2)",
  display: "grid",
  placeItems: "center",
  color: "var(--muted2)",
  cursor: "pointer",
  fontSize: 13,
};

export default function SettingsPage() {
  const { t, i18n } = useTranslation();

  const identity = useData((s) => s.identity);
  const settings = useData((s) => s.settings);
  const downloadDir = useData((s) => s.downloadDir);
  const networkInfo = useData((s) => s.networkInfo);
  const listenPort = useData((s) => s.listenPort);
  const setDeviceName = useData((s) => s.setDeviceName);
  const setAutoOpen = useData((s) => s.setAutoOpen);
  const setDownloadDir = useData((s) => s.setDownloadDir);
  const setPair = useOverlays((s) => s.setPair);
  const setLicense = useOverlays((s) => s.setLicense);

  const prefs = usePrefs();
  const set = prefs.set;

  const vis: Visibility = visibilityOf(settings, prefs.ghostUntil);
  const deviceName = settings?.deviceName ?? identity?.name ?? "";
  const nameInitial = (deviceName || "L").trim().charAt(0) || "L";

  const [editingName, setEditingName] = useState(false);
  const nameCancel = useRef(false);

  const commitName = (value: string) => {
    if (nameCancel.current) {
      nameCancel.current = false;
      setEditingName(false);
      return;
    }
    const v = value.trim();
    if (v && v !== deviceName) void setDeviceName(v);
    setEditingName(false);
  };

  const milestone = () => showToast(t("common.milestoneNote"));

  // 全局快捷键改键 (M5.5 rebind): 修改 focuses the chip into a capture state; the
  // next modifier+key chord is bound live via set_hotkey. The stored value is the
  // canonical accelerator (prefs.hotkey mirror); the chip shows its platform label.
  const [capturing, setCapturing] = useState(false);
  const [capture, setCapture] = useState("");
  const captureRef = useRef<HTMLButtonElement>(null);
  // Focus the chip when capture starts so it receives the keydown.
  useEffect(() => {
    if (capturing) captureRef.current?.focus();
  }, [capturing]);

  const commitHotkey = (combo: string) => {
    setCapturing(false);
    setCapture("");
    const prev = prefs.hotkey || "Alt+Space";
    if (combo === prev) return; // unchanged — nothing to rebind
    // Optimistic mirror so the chip shows the new chord at once; roll back if the
    // backend can't bind it (another app / the OS already owns the chord).
    set({ hotkey: combo });
    api
      .setHotkey(combo)
      .then(() => {
        // Adopt into the backend settings snapshot too (like commitPort).
        useData.setState((s) => ({
          settings: s.settings && { ...s.settings, hotkey: combo },
        }));
        showToast(t("settings.hotkeyChanged"));
      })
      .catch(() => {
        set({ hotkey: prev });
        showToast(t("settings.hotkeyTaken"));
      });
  };

  const onHotkeyKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      setCapturing(false);
      setCapture("");
      return;
    }
    const mods = heldMods(e);
    const token = MOD_CODES.has(e.code) ? null : keyToken(e.code);
    // Live preview while the chord is being built.
    setCapture([...mods, ...(token ? [token] : [])].join("+"));
    // Commit only a full chord: at least one modifier + a bindable key.
    if (token && mods.length > 0) commitHotkey([...mods, token].join("+"));
  };

  const cancelCapture = () => {
    setCapturing(false);
    setCapture("");
  };
  const startCapture = () => {
    setCapture("");
    setCapturing(true);
  };
  // The chip's live text: the prompt (or preview) while capturing, else the
  // formatted current chord.
  const hotkeyDisplay = capturing
    ? capture
      ? formatHotkey(capture)
      : t("settings.hotkeyCapture")
    : formatHotkey(prefs.hotkey || "Alt+Space");

  // The desired port after a restart: 0 is the "follow the default" sentinel,
  // and LanBeam's default TCP port is 51704 (M5.2).
  const effPort = settings?.port || 51704;
  // The port the listener is bound to RIGHT NOW — it lags effPort until a
  // restart, or is an ephemeral fallback if the configured port was taken.
  // Fall back to the desired port when the backend value is unavailable
  // (browser demo / before the first load).
  const boundPort = listenPort || effPort;
  // Honor the pinned interface (M5.6) over the numerically-first entry, which
  // is often a VPN/virtual adapter LAN peers can't reach.
  const localIp = displayIp(networkInfo, prefs.iface);
  // Identity line shows the LIVE port; when a pending change hasn't taken
  // effect yet (needs a restart) append it so a firewall rule / IP-direct
  // dial targets the right one.
  const portLabel =
    boundPort !== effPort
      ? `${t("settings.portShort", { port: boundPort })} · ${t("settings.portPending", { port: effPort })}`
      : t("settings.portShort", { port: boundPort });

  // Keep the interface list / bound-IP fresh while the app is open, not just
  // at the once-per-launch startup fetch (M5.6): interfaces change under a
  // long-lived tray session.
  useEffect(() => {
    void useData.getState().refreshNetworkInfo();
  }, []);

  // 下载位置 (M5.2): system directory picker → backend canonicalizes + stores.
  const changeDownloadDir = async () => {
    if (!api.isTauri) {
      milestone();
      return;
    }
    const { open } = await import("@tauri-apps/plugin-dialog");
    const dir = await open({
      directory: true,
      title: t("settings.downloadDir"),
    });
    if (typeof dir !== "string" || !dir) return;
    try {
      await setDownloadDir(dir);
      showToast(t("settings.dirChangedToast"));
    } catch (e) {
      showToast(errText(e));
    }
  };

  // 监听端口 (M5.2): the input edits the pref mirror; Enter/blur commits.
  const commitPort = () => {
    const n = parseInt(prefs.port, 10);
    // Empty/0 = back to the default; the backend refuses privileged ports,
    // so mirror that gate here instead of letting the save silently no-op.
    if (prefs.port !== "" && n !== 0 && (n < 1024 || n > 65535)) {
      set({ port: String(effPort) });
      showToast(t("settings.portInvalid"));
      return;
    }
    const commit = prefs.port === "" || n === 0 ? 0 : n;
    // No-change guard (compare parsed numbers, not raw strings): either the
    // value already matches the stored one, or the stored value is the 0
    // sentinel and the field still shows the effective default — the case a
    // mere focus/blur of the untouched field hits. Pinning 51704 over the
    // sentinel would silently stop it following DEFAULT_TCP_PORT across
    // upgrades; the field can't express "pin 51704" vs "follow default"
    // anyway (both render 51704), and clearing the field still reaches 0.
    const stored = settings?.port ?? 0;
    if (commit === stored || (stored === 0 && commit === effPort)) {
      set({ port: String(effPort) }); // re-canonicalize e.g. "051704"
      return;
    }
    set({ port: String(commit || 51704) });
    void api.setListenPort(commit);
    useData.setState((s) => ({
      settings: s.settings && { ...s.settings, port: commit },
    }));
    showToast(t("settings.portSavedToast"));
  };

  // 重置本机身份 (M5.7): two-step inline confirm — the first click arms the
  // button (3 s auto-revert), the second actually resets + restarts.
  const [resetArmed, setResetArmed] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  // Wall-clock when the button armed — a ref so the guard reads it synchronously
  // (state would lag the same-tick second click of a double-click).
  const resetArmedAt = useRef(0);
  useEffect(() => () => clearTimeout(resetTimer.current), []);
  const onResetClick = () => {
    if (!resetArmed) {
      setResetArmed(true);
      resetArmedAt.current = Date.now();
      clearTimeout(resetTimer.current);
      resetTimer.current = setTimeout(() => setResetArmed(false), 3000);
      return;
    }
    // Swallow the confirming click of an accidental double-click (~100–200 ms
    // apart, a common Windows habit): stay armed, keep the 3 s auto-revert
    // running, and let only a deliberate second click after the window reset.
    if (Date.now() - resetArmedAt.current < 350) return;
    clearTimeout(resetTimer.current);
    setResetArmed(false);
    // Success never resolves (the backend restarts the app) — only failures
    // come back, and they leave the identity intact (see reset_identity).
    api.resetIdentity().catch((e) => showToast(errText(e)));
  };

  // 日志文件 row (M4.6): both actions are real now — open the backend's log
  // directory / write a diagnostics bundle and reveal it.
  const openLogDir = () => {
    void api
      .getLogDir()
      .then((dir) => openDir(dir))
      .catch((e) => showToast(errText(e)));
  };
  const exportDiag = () => {
    void api
      .exportDiagnostics()
      .then((path) => {
        showToast(t("settings.diagExportedToast", { path }));
        return revealFile(path);
      })
      .catch((e) => showToast(errText(e)));
  };

  const onVisChange = (v: Visibility) => {
    void setVisibility(v);
    if (v === "ghost") showToast(t("vis.ghostToast"));
  };

  const copyFp = () => {
    const id = identity?.deviceId;
    if (!id) return;
    copyText(id);
    showToast(t("settings.fpCopied"));
  };

  const pill: { fg: string; bg: string; label: string } =
    vis === "on"
      ? {
          fg: "var(--success)",
          bg: "var(--success-soft)",
          label: t("settings.statusOn"),
        }
      : vis === "ghost"
        ? {
            fg: "var(--accent-ink)",
            bg: "var(--accent-soft)",
            label: t("settings.statusGhost"),
          }
        : {
            fg: "var(--muted)",
            bg: "var(--track)",
            label: t("settings.statusOff"),
          };

  const lang = i18n.language.startsWith("zh") ? "zh" : "en";

  return (
    <div
      className="scroll-y"
      style={{ flex: 1, animation: "lbFade .18s ease" }}
    >
      <div
        style={{ maxWidth: 620, margin: "0 auto", padding: "22px 24px 34px" }}
      >
        {/* ── identity card ─────────────────────────────────────────── */}
        <div
          className="card"
          style={{ position: "relative", borderRadius: 14 }}
        >
          <div
            style={{
              position: "absolute",
              left: -54,
              top: -52,
              width: 200,
              height: 200,
              border: "1px solid var(--ring-a)",
              borderRadius: "50%",
            }}
          />
          <div
            style={{
              position: "absolute",
              left: -94,
              top: -92,
              width: 280,
              height: 280,
              border: "1px solid var(--ring-b)",
              borderRadius: "50%",
            }}
          />
          <div
            style={{
              position: "absolute",
              left: -134,
              top: -132,
              width: 360,
              height: 360,
              border: "1px solid var(--ring-c)",
              borderRadius: "50%",
            }}
          />
          <div
            style={{
              position: "relative",
              display: "flex",
              gap: 14,
              padding: 16,
              alignItems: "center",
            }}
          >
            <div
              style={{
                width: 60,
                height: 60,
                borderRadius: "50%",
                background: "var(--panel)",
                border: "1px solid var(--border2)",
                boxShadow: "0 0 24px var(--ring-a)",
                display: "grid",
                placeItems: "center",
                fontSize: 22,
                fontWeight: 650,
                color: "var(--accent-ink)",
                flex: "none",
              }}
            >
              {nameInitial}
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              {editingName ? (
                <input
                  defaultValue={deviceName}
                  // biome-ignore lint/a11y/noAutofocus: the rename/edit field autofocuses when opened (deliberate UX)
                  autoFocus
                  placeholder={t("settings.renamePlaceholder")}
                  onBlur={(e) => commitName(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    /* IME guard: Enter/Escape while composing only acts on the
                       candidate window (keyCode 229 covers WKWebView, where the
                       Enter can fire after compositionend with isComposing false) */
                    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
                    if (e.key === "Enter") e.currentTarget.blur();
                    if (e.key === "Escape") {
                      nameCancel.current = true;
                      e.currentTarget.blur();
                    }
                  }}
                  style={{
                    height: 30,
                    width: 230,
                    padding: "0 10px",
                    borderRadius: 8,
                    border: "1px solid var(--accent)",
                    background: "var(--bg)",
                    color: "var(--ink)",
                    fontSize: 13,
                    fontWeight: 600,
                    fontFamily: "inherit",
                    outline: "none",
                  }}
                />
              ) : (
                <div
                  style={{ display: "flex", alignItems: "baseline", gap: 9 }}
                >
                  <span
                    style={{
                      fontSize: 15,
                      fontWeight: 650,
                      color: "var(--ink2)",
                    }}
                  >
                    {deviceName}
                  </span>
                  {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={() => setEditingName(true)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        if (e.key === " ") e.preventDefault();
                        setEditingName(true);
                      }
                    }}
                    style={{
                      fontSize: 11,
                      color: "var(--accent-ink)",
                      cursor: "pointer",
                    }}
                  >
                    {t("common.rename")}
                  </span>
                </div>
              )}
              <div
                style={{
                  fontFamily: "var(--mono)",
                  fontSize: 11,
                  color: "var(--muted2)",
                  marginTop: 4,
                }}
              >
                {identity?.shortId ?? ""}
                {/* real endpoint (M5.1): the mock reserved this mono slot.
                    Port is the actually-bound one (M5.2), not settings.port. */}
                {localIp ? ` · ${localIp} · ${portLabel}` : ""}
              </div>
              <div
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: 8,
                  marginTop: 8,
                  background: "var(--accent-soft)",
                  borderRadius: 8,
                  padding: "4px 9px",
                }}
              >
                <span
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 10.5,
                    fontWeight: 600,
                    color: "var(--accent-ink)",
                  }}
                >
                  {fpGroups(identity?.deviceId ?? "")}
                </span>
                {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                <span
                  role="button"
                  tabIndex={0}
                  onClick={copyFp}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      if (e.key === " ") e.preventDefault();
                      copyFp();
                    }
                  }}
                  style={{
                    fontSize: 10.5,
                    color: "var(--accent-ink)",
                    cursor: "pointer",
                  }}
                >
                  {t("settings.copyFp")}
                </span>
                {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                <span
                  role="button"
                  tabIndex={0}
                  onClick={() => setPair(true)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      if (e.key === " ") e.preventDefault();
                      setPair(true);
                    }
                  }}
                  style={{
                    fontSize: 10.5,
                    color: "var(--accent-ink)",
                    cursor: "pointer",
                  }}
                >
                  {t("settings.qrCode")}
                </span>
              </div>
            </div>
            <span
              style={{
                flex: "none",
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                fontSize: 10.5,
                fontWeight: 600,
                color: pill.fg,
                background: pill.bg,
                borderRadius: 99,
                padding: "4px 11px",
              }}
            >
              <span
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: "50%",
                  background: pill.fg,
                }}
              />
              {pill.label}
            </span>
          </div>
        </div>

        {/* ── 通用 ──────────────────────────────────────────────────── */}
        <div className="set-section" style={{ paddingTop: 22 }}>
          {t("settings.secGeneral")}
        </div>
        <div className="set-row">
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="set-label">{t("settings.downloadDir")}</div>
            <div
              className="set-desc mono"
              title={downloadDir}
              style={{
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {downloadDir}
            </div>
          </div>
          <button
            type="button"
            className="btn sm ink"
            style={{ flex: "none" }}
            onClick={() => void changeDownloadDir()}
          >
            {t("common.change")}
          </button>
        </div>
        <div className="set-row last">
          <div>
            <div className="set-label">{t("settings.visibility")}</div>
            <div className="set-desc">
              {vis === "on"
                ? t("settings.visOnDesc")
                : vis === "ghost"
                  ? t("settings.visGhostDesc")
                  : t("settings.visOffDesc")}
            </div>
          </div>
          <select
            className="input"
            style={{ flex: "none" }}
            value={vis}
            onChange={(e) => onVisChange(e.target.value as Visibility)}
          >
            <option value="on">{t("settings.visOn")}</option>
            <option value="ghost">{t("settings.visGhost")}</option>
            <option value="off">{t("settings.visOff")}</option>
          </select>
        </div>

        {/* ── 接收与文件 ─────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secRecv")}</div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.recvPolicy")}</div>
            <div className="set-desc">{t("settings.recvPolicyDesc")}</div>
          </div>
          <select
            className="input"
            value={prefs.recvPolicy}
            onChange={(e) => {
              const v = e.target.value;
              // Pref mirror keeps the select instant; the backend copy is what
              // the receive path actually consults (M4.4).
              set({ recvPolicy: v });
              void api.setRecvPolicy(v);
            }}
          >
            <option value="ask">{t("settings.recvAsk")}</option>
            <option value="trusted">{t("settings.recvTrusted")}</option>
            <option value="all">{t("settings.recvAll")}</option>
          </select>
        </div>
        <div className="set-row">
          <span className="set-label">{t("settings.conflict")}</span>
          <select
            className="input"
            value={prefs.conflict}
            onChange={(e) => {
              const v = e.target.value;
              // Mirror + write-through (M6.5): the receive path reads the backend
              // copy at receive time; "ask" drives the ConflictModal.
              set({ conflict: v });
              void api.setConflictPolicy(v);
            }}
          >
            <option value="rename">{t("settings.conflictRename")}</option>
            <option value="overwrite">{t("settings.conflictOverwrite")}</option>
            <option value="ask">{t("settings.conflictAsk")}</option>
          </select>
        </div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.organize")}</div>
            <div className="set-desc">{t("settings.organizeDesc")}</div>
          </div>
          <select
            className="input"
            value={prefs.organize}
            onChange={(e) => {
              const v = e.target.value;
              // Mirror + write-through (M6.6): the receive path computes the
              // subfolder from the backend copy when a transfer starts.
              set({ organize: v });
              void api.setOrganize(v);
            }}
          >
            <option value="none">{t("settings.organizeNone")}</option>
            <option value="device">{t("settings.organizeDevice")}</option>
            <option value="date">{t("settings.organizeDate")}</option>
          </select>
        </div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.autoOpen")}</div>
            <div className="set-desc">{t("settings.autoOpenDesc")}</div>
          </div>
          <Toggle
            on={settings?.autoOpenFolder ?? false}
            onClick={() => {
              if (settings) void setAutoOpen(!settings.autoOpenFolder);
            }}
          />
        </div>
        <div className="set-row last">
          <div>
            <div className="set-label">{t("settings.verify")}</div>
            <div className="set-desc">{t("settings.verifyDesc")}</div>
          </div>
          <Toggle
            on={prefs.verifyHash}
            onClick={() => {
              const v = !prefs.verifyHash;
              // Mirror for instant UI; the backend reads its copy at send
              // time (M6.3), so the toggle takes effect on the next transfer.
              set({ verifyHash: v });
              void api.setVerifyHash(v);
            }}
          />
        </div>

        {/* ── 行为与通知 ─────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secBehavior")}</div>
        <div className="set-row">
          <span className="set-label">{t("settings.autoStart")}</span>
          <Toggle
            on={prefs.autoStart}
            onClick={() => {
              const v = !prefs.autoStart;
              // Optimistic mirror; set_autostart can fail (the OS refused the
              // login entry, M5.5) — roll back so the toggle never lies.
              set({ autoStart: v });
              api.setAutostart(v).catch((e) => {
                set({ autoStart: !v });
                showToast(errText(e));
              });
            }}
          />
        </div>
        <div className="set-row">
          <span className="set-label">{t("settings.trayClose")}</span>
          <Toggle
            on={prefs.trayClose}
            onClick={() => {
              const v = !prefs.trayClose;
              // Same mirror pattern as recvPolicy — the close handler reads
              // the backend copy live (M5.3).
              set({ trayClose: v });
              void api.setTrayClose(v);
            }}
          />
        </div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.notifSys")}</div>
            <div className="set-desc">{t("settings.notifSysDesc")}</div>
          </div>
          <Toggle
            on={prefs.notifSys}
            onClick={() => {
              const v = !prefs.notifSys;
              set({ notifSys: v });
              void api.setNotifSystem(v);
            }}
          />
        </div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.sound")}</div>
            <div className="set-desc">{t("settings.soundDesc")}</div>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <select
              className="input"
              value={prefs.soundKind}
              onChange={(e) => {
                const v = e.target.value as SoundKind;
                set({ soundKind: v });
                playSound(v);
              }}
            >
              {SOUNDS.map((s) => (
                <option key={s.v} value={s.v}>
                  {t(s.k)}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn sm ink"
              style={{ padding: "0 12px" }}
              onClick={() => playSound(prefs.soundKind)}
            >
              {t("settings.soundTry")}
            </button>
            <Toggle
              on={prefs.notifSound}
              onClick={() => {
                const v = !prefs.notifSound;
                set({ notifSound: v });
                if (v) playSound(prefs.soundKind);
              }}
            />
          </div>
        </div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.clipShare")}</div>
            <div className="set-desc">{t("settings.clipShareDesc")}</div>
          </div>
          <Toggle
            on={prefs.clipShare}
            onClick={() => {
              const v = !prefs.clipShare;
              // Mirror for instant UI; the backend reads its copy when a text
              // arrives (M7.3), so it applies to the very next one.
              set({ clipShare: v });
              void api.setClipShare(v);
            }}
          />
        </div>
        <div className="set-row last">
          <span className="set-label">{t("settings.hotkey")}</span>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            {/* The chip captures the new chord: clicking it (or 修改) starts
                capture, the next modifier+key commits, Escape / blur cancels. */}
            <button
              type="button"
              ref={captureRef}
              onClick={capturing ? undefined : startCapture}
              onKeyDown={capturing ? onHotkeyKeyDown : undefined}
              onBlur={capturing ? cancelCapture : undefined}
              style={{
                fontFamily: "var(--mono)",
                fontSize: 11,
                color: capturing ? "var(--accent-ink)" : "var(--muted2)",
                background: capturing ? "var(--accent-soft)" : "var(--sidebar)",
                border: `1px solid ${capturing ? "var(--accent)" : "var(--border)"}`,
                borderRadius: 6,
                padding: "3px 9px",
                minWidth: 78,
                textAlign: "center",
                cursor: capturing ? "default" : "pointer",
              }}
            >
              {hotkeyDisplay}
            </button>
            <button
              type="button"
              className="btn sm ink"
              onClick={capturing ? cancelCapture : startCapture}
            >
              {capturing ? t("common.cancel") : t("settings.hotkeyChange")}
            </button>
            <Toggle
              on={prefs.hotkeyEnabled}
              onClick={() => {
                const v = !prefs.hotkeyEnabled;
                // Mirror + write-through (M5.5): the backend (un)registers the
                // configured chord live and can't fail the call, so no rollback.
                set({ hotkeyEnabled: v });
                void api.setHotkeyEnabled(v);
              }}
            />
          </div>
        </div>

        {/* ── 网络 ──────────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secNetwork")}</div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.port")}</div>
            <div className="set-desc">
              {t("settings.portDesc")} · {t("settings.logRestartNote")}
            </div>
          </div>
          <input
            className="input"
            value={prefs.port}
            onChange={(e) =>
              set({ port: e.target.value.replace(/[^0-9]/g, "").slice(0, 5) })
            }
            onBlur={commitPort}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
            }}
            style={{ width: 86, fontFamily: "var(--mono)", textAlign: "right" }}
          />
        </div>
        <div className="set-row">
          <span className="set-label">{t("settings.iface")}</span>
          <select
            className="input"
            value={prefs.iface}
            onChange={(e) => {
              const v = e.target.value;
              // Mirror + write-through (M5.6): the announce loop re-reads the
              // filter within one 2 s tick, no restart needed.
              set({ iface: v });
              void api.setIfaceFilter(v);
            }}
          >
            <option value="">{t("settings.ifaceAuto")}</option>
            {networkInfo.map((n) => (
              <option key={n.ip} value={n.ip}>
                {n.ip}
              </option>
            ))}
            {/* a stored filter whose interface vanished still needs an option,
                or the select would silently show 自动选择 for a live filter */}
            {prefs.iface && !networkInfo.some((n) => n.ip === prefs.iface) && (
              <option value={prefs.iface}>{prefs.iface}</option>
            )}
          </select>
        </div>
        {/* The former「mDNS 发现」row is gone (M5.8): discovery is LanBeam's
            own UDP announce — not mDNS — and its on/off is exactly the
            visibility select above, so a second switch could only contradict
            it. usePrefs.mdns stays in the persisted blob, unrendered. */}
        <div className="set-row">
          <span className="set-label">{t("settings.concurrent")}</span>
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            {/* biome-ignore lint/a11y/useSemanticElements: styled stepper control, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
            <span
              className="hover-row"
              style={stepBtn}
              role="button"
              tabIndex={0}
              onClick={() => {
                // Mirror + write-through (M6.7); the backend clamps to 1–8 and
                // reads the value live at each transfer's concurrency gate.
                const n = Math.max(1, prefs.concurrent - 1);
                set({ concurrent: n });
                void api.setMaxConcurrent(n);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  if (e.key === " ") e.preventDefault();
                  const n = Math.max(1, prefs.concurrent - 1);
                  set({ concurrent: n });
                  void api.setMaxConcurrent(n);
                }
              }}
            >
              −
            </span>
            <span
              style={{
                fontFamily: "var(--mono)",
                fontSize: 12.5,
                color: "var(--ink2)",
                minWidth: 14,
                textAlign: "center",
              }}
            >
              {prefs.concurrent}
            </span>
            {/* biome-ignore lint/a11y/useSemanticElements: styled stepper control, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
            <span
              className="hover-row"
              style={stepBtn}
              role="button"
              tabIndex={0}
              onClick={() => {
                const n = Math.min(8, prefs.concurrent + 1);
                set({ concurrent: n });
                void api.setMaxConcurrent(n);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  if (e.key === " ") e.preventDefault();
                  const n = Math.min(8, prefs.concurrent + 1);
                  set({ concurrent: n });
                  void api.setMaxConcurrent(n);
                }
              }}
            >
              ＋
            </span>
          </div>
        </div>
        <div className="set-row">
          <span className="set-label">{t("settings.rate")}</span>
          <select
            className="input"
            value={prefs.rate}
            onChange={(e) => {
              const v = e.target.value;
              // Mirror + write-through (M6.7); the backend reads the cap when a
              // transfer starts streaming, applying it from the next one on.
              set({ rate: v });
              void api.setRateLimit(v);
            }}
          >
            <option value="unlimited">{t("settings.rateUnlimited")}</option>
            <option value="50">50 MB/s</option>
            <option value="10">10 MB/s</option>
          </select>
        </div>
        <div className="set-row last">
          <div>
            <div className="set-label">{t("settings.ssid")}</div>
            <div className="set-desc">{t("settings.ssidDesc")}</div>
          </div>
          <select
            className="input"
            value={prefs.ssidOnly}
            onChange={(e) => set({ ssidOnly: e.target.value })}
          >
            <option value="any">{t("settings.ssidAny")}</option>
            <option value="current">{t("settings.ssidCurrent")}</option>
          </select>
        </div>

        {/* ── 外观 ──────────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secAppearance")}</div>
        <div className="set-row">
          <span className="set-label">{t("settings.theme")}</span>
          <Segmented<ThemeMode>
            options={[
              { key: "light", label: t("settings.themeLight") },
              { key: "dark", label: t("settings.themeDark") },
              { key: "system", label: t("settings.themeSystem") },
            ]}
            value={prefs.themeMode}
            onChange={(k) => set({ themeMode: k })}
            itemStyle={{ padding: "4px 14px" }}
          />
        </div>
        <div className="set-row last">
          <span className="set-label">{t("settings.language")}</span>
          <select
            className="input"
            value={lang}
            onChange={(e) => void i18n.changeLanguage(e.target.value)}
          >
            <option value="zh">中文（简体）</option>
            <option value="en">English</option>
          </select>
        </div>

        {/* ── 隐私与安全 ─────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secPrivacy")}</div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.exif")}</div>
            <div className="set-desc">{t("settings.exifDesc")}</div>
          </div>
          <Toggle
            on={prefs.stripExif}
            onClick={() => {
              const v = !prefs.stripExif;
              // Mirror for instant UI + the confirm-modal default; the backend
              // reads its copy at send time (M9.1), so it applies to the very
              // next transfer.
              set({ stripExif: v });
              void api.setStripExif(v);
            }}
          />
        </div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.histKeep")}</div>
            <div className="set-desc">{t("settings.histKeepDesc")}</div>
          </div>
          <select
            className="input"
            value={prefs.histKeep}
            onChange={(e) => set({ histKeep: e.target.value })}
          >
            <option value="none">{t("settings.histNone")}</option>
            <option value="7d">{t("settings.hist7")}</option>
            <option value="30d">{t("settings.hist30")}</option>
            <option value="forever">{t("settings.histForever")}</option>
          </select>
        </div>
        <div className="set-row last">
          <div>
            <div className="set-label">{t("settings.resetId")}</div>
            <div className="set-desc">{t("settings.resetIdDesc")}</div>
          </div>
          <button
            type="button"
            className="btn sm danger"
            style={
              resetArmed
                ? {
                    background: "var(--danger)",
                    borderColor: "var(--danger)",
                    color: "#fff",
                  }
                : undefined
            }
            onClick={onResetClick}
          >
            {resetArmed ? t("settings.resetConfirm") : t("settings.resetBtn")}
          </button>
        </div>

        <div
          style={{
            fontSize: 11,
            color: "var(--muted)",
            lineHeight: 1.7,
            padding: "12px 0 2px",
          }}
        >
          {t("settings.e2eNote")}
        </div>

        {/* ── 日志 ──────────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secLogs")}</div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.logLevel")}</div>
            <div className="set-desc">
              {t("settings.logLevelDesc")} · {t("settings.logRestartNote")}
            </div>
          </div>
          <select
            className="input"
            value={prefs.logLevel}
            onChange={(e) => {
              const v = e.target.value;
              // Same mirror pattern as recvPolicy; the file logger picks the
              // stored level up on the next launch (M4.6).
              set({ logLevel: v });
              void api.setLogLevel(v);
            }}
          >
            <option value="errors">{t("settings.logErrors")}</option>
            <option value="normal">{t("settings.logNormal")}</option>
            <option value="verbose">{t("settings.logVerbose")}</option>
          </select>
        </div>
        <div className="set-row last">
          <span className="set-label">{t("settings.logFiles")}</span>
          <div style={{ display: "flex", gap: 8, flex: "none" }}>
            <button type="button" className="btn sm ink" onClick={openLogDir}>
              {t("settings.openLogDir")}
            </button>
            <button type="button" className="btn sm ink" onClick={exportDiag}>
              {t("settings.exportDiag")}
            </button>
          </div>
        </div>

        {/* ── 关于 ──────────────────────────────────────────────────── */}
        <div className="set-section">{t("settings.secAbout")}</div>
        <div className="set-row">
          <div>
            <div className="set-label">{t("settings.version")}</div>
            <div
              className="set-desc"
              style={{ fontFamily: "var(--mono)", fontSize: 11 }}
            >
              LanBeam v0.1.0
            </div>
          </div>
          <button
            type="button"
            className="btn sm ink"
            onClick={() => showToast(t("settings.updateManual"))}
          >
            {t("settings.checkUpdate")}
          </button>
        </div>
        <div className="set-row last">
          <div>
            <div className="set-label">{t("settings.license")}</div>
            <div className="set-desc">{t("settings.licenseDesc")}</div>
          </div>
          <button
            type="button"
            className="btn sm ink"
            onClick={() => setLicense(true)}
          >
            {t("common.view")}
          </button>
        </div>
      </div>
    </div>
  );
}
