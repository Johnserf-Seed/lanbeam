import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  isTauri,
  listShares,
  onEvent,
  type ShareDownloadEvent,
  type ShareEntry,
  startShare,
  stopShare,
  updateShare,
} from "../bridge/api";
import { fmtBytes } from "../lib/format";
import { copyText, errText, pickFiles } from "../lib/sendops";
import { showToast, useOverlays, type SendFile } from "../lib/store";
import Qr from "./Qr";
import { ModalHead } from "./ui";

function MenuItem({
  label,
  active,
  onPick,
}: {
  label: string;
  active: boolean;
  onPick: () => void;
}) {
  return (
    // biome-ignore lint/a11y/useSemanticElements: styled menu-item row, not a native button — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown
    <div
      role="button"
      tabIndex={0}
      onClick={onPick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onPick();
        }
      }}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "8px 12px",
        fontSize: 11.5,
        fontWeight: 600,
        color: active ? "var(--accent-ink)" : "var(--ink2)",
        background: active ? "var(--accent-soft)" : "transparent",
        cursor: "pointer",
        whiteSpace: "nowrap",
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = "var(--hover)")}
      onMouseLeave={(e) =>
        (e.currentTarget.style.background = active
          ? "var(--accent-soft)"
          : "transparent")
      }
    >
      <span style={{ flex: 1 }}>{label}</span>
      <span style={{ fontSize: 10, color: "var(--accent-ink)" }}>
        {active ? "✓" : ""}
      </span>
    </div>
  );
}

function DropPill({
  text,
  open,
  onToggle,
  minWidth,
  options,
  activeIdx,
  onPick,
}: {
  text: string;
  open: boolean;
  onToggle: () => void;
  minWidth: number;
  options: string[];
  activeIdx: number;
  onPick: (i: number) => void;
}) {
  return (
    <div style={{ position: "relative" }}>
      {/* biome-ignore lint/a11y/useSemanticElements: styled dropdown pill, not a native button — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
      <span
        role="button"
        tabIndex={0}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 5,
          height: 26,
          boxSizing: "border-box",
          fontSize: 10.5,
          background: "var(--accent-soft)",
          color: "var(--accent-ink)",
          borderRadius: 99,
          padding: "0 11px",
          fontWeight: 600,
          cursor: "pointer",
          whiteSpace: "nowrap",
        }}
      >
        {text}
        <span style={{ fontSize: 8, lineHeight: 1 }}>▾</span>
      </span>
      {open && (
        <div
          style={{
            position: "absolute",
            left: 0,
            bottom: 32,
            zIndex: 6,
            minWidth,
            background: "var(--panel)",
            border: "1px solid var(--border2)",
            borderRadius: 10,
            boxShadow: "var(--shadow)",
            overflow: "hidden",
            animation: "lbUp .15s ease",
          }}
        >
          {options.map((label, i) => (
            <MenuItem
              key={label}
              label={label}
              active={i === activeIdx}
              onPick={() => onPick(i)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** Lifetime dropdown → link TTL in seconds (10 min / 1 h / 24 h). */
const LIFE_SECS = [600, 3600, 86_400];
/** Download-count dropdown → maxDownloads (1 / 3 / unlimited). */
const MAX_DL: (number | null)[] = [1, 3, null];

/** 用浏览器接收 (M8.2): registers the send flow's selected files as a real LAN
 *  browser share and surfaces the link + QR + lifetime/count controls. Closing
 *  the modal WITHOUT pressing 停止分享 leaves the share live until its TTL — the
 *  link keeps working, and a future shares list can reopen it via `list_shares`.
 *  Falls back to an honest note when there is nothing path-backed to serve (a
 *  browser demo with no file server, or a selection of size-only entries). */
export default function ShareModal() {
  const { t } = useTranslation();
  const shareOpen = useOverlays((s) => s.shareOpen);
  const setShare = useOverlays((s) => s.setShare);
  const send = useOverlays((s) => s.send);

  const [link, setLink] = useState<{ token: string; url: string } | null>(null);
  const [entry, setEntry] = useState<ShareEntry | null>(null);
  const [sharedFiles, setSharedFiles] = useState<SendFile[]>([]);
  const [noPaths, setNoPaths] = useState(false);
  const [startError, setStartError] = useState(false);
  const [lifeIdx, setLifeIdx] = useState(0);
  const [onceIdx, setOnceIdx] = useState(0);
  const [menu, setMenu] = useState<"life" | "once" | null>(null);
  // True only when the pointer went down on the scrim itself; a drag that
  // starts inside the modal and ends on the scrim dispatches click on the
  // scrim, which must not dismiss the modal.
  const scrimDown = useRef(false);
  // Guards the once-per-open share creation against re-renders / strict mode.
  const startedRef = useRef(false);

  // On open: register ONE fresh share for the current selection (or a picked
  // set). On close: reset the guard but DELIBERATELY do not stop the share — it
  // lives out its TTL so the link a recipient already has keeps working.
  useEffect(() => {
    if (!shareOpen) {
      startedRef.current = false;
      return;
    }
    if (startedRef.current) return;
    startedRef.current = true;
    setLink(null);
    setEntry(null);
    setSharedFiles([]);
    setNoPaths(false);
    setStartError(false);
    setMenu(null);
    setLifeIdx(0);
    setOnceIdx(0);

    let cancelled = false;
    void (async () => {
      // Prefer the send flow's current selection (path-backed only); if no send
      // flow is open, let the user pick files (native picker, desktop only).
      const ov = useOverlays.getState().send;
      let files: SendFile[] = ov
        ? ov.pool.filter((f) => f.path && ov.sel.includes(f.path ?? f.name))
        : [];
      if (files.length === 0 && isTauri) {
        files = (await pickFiles(t("share.pickTitle"))).filter((f) => f.path);
      }
      if (cancelled) return;
      const paths = files.flatMap((f) => (f.path ? [f.path] : []));
      if (paths.length === 0) {
        // Never register an empty share: close on a dismissed desktop picker,
        // else surface the honest browser-demo note (no local file server).
        if (isTauri) setShare(false);
        else setNoPaths(true);
        return;
      }
      setSharedFiles(files);
      try {
        const res = await startShare(paths, LIFE_SECS[0], MAX_DL[0]);
        if (cancelled) return;
        setLink({ token: res.token, url: res.url });
        // Read the file count + total size back from the server (it stat'd the
        // files) so the subtitle is accurate even for a freshly-picked set.
        const list = await listShares().catch(() => [] as ShareEntry[]);
        if (!cancelled)
          setEntry(list.find((e) => e.token === res.token) ?? null);
      } catch (e) {
        if (cancelled) return;
        // A genuine backend failure in the desktop app (file moved/deleted, a
        // folder was picked, the share server never bound) — NOT the browser
        // demo's no-file-server case. Surface a real error, never the demoNote.
        setStartError(true);
        showToast(t("share.startFailedToast", { err: errText(e) }));
      }
    })();
    return () => {
      cancelled = true;
    };
    // Runs only on open/close; reads the freshest selection via getState inside,
    // so `send`/`t`/`setShare` are intentionally not dependencies.
  }, [shareOpen]);

  // Live download count: while the panel is open, bump the displayed count when
  // a browser fetches a file from THIS share. The event carries the whole-set
  // count, so set it verbatim.
  useEffect(() => {
    const token = link?.token;
    if (!shareOpen || !token) return;
    return onEvent<ShareDownloadEvent>("share_download", (ev) => {
      if (ev.token !== token) return;
      setEntry((e) =>
        e
          ? {
              ...e,
              // Downloads only ever climb; clamp so an out-of-order event (two
              // concurrent fetches finishing in reverse) can't tick it backward.
              downloads: Math.max(e.downloads, ev.downloads),
              maxDownloads: ev.maxDownloads,
            }
          : e,
      );
    });
  }, [shareOpen, link?.token]);

  if (!shareOpen) return null;
  const close = () => setShare(false);

  // Subtitle: the server's authoritative file count + size once the share
  // exists, else the pending selection so the header isn't blank while loading.
  const pending = send
    ? send.pool.filter((f) => send.sel.includes(f.path ?? f.name))
    : [];
  const shown = sharedFiles.length ? sharedFiles : pending;
  const sub = t("share.sub", {
    n: entry?.fileCount ?? shown.length,
    size: entry
      ? fmtBytes(entry.totalSize)
      : fmtBytes(shown.reduce((a, f) => a + (f.size ?? 0), 0)),
  });

  const lifeOpts = [t("share.life10m"), t("share.life1h"), t("share.life24h")];
  const onceOpts = [t("share.once1"), t("share.once3"), t("share.onceAny")];

  const copyLink = () => {
    if (!link) return;
    copyText(link.url);
    showToast(t("share.copiedToast"));
  };

  // A dropdown change reconfigures the live share in place; both controls ride
  // update_share together because the backend takes the full config each call.
  const applyConfig = (life: number, once: number) => {
    const l = link;
    if (!l) return;
    void (async () => {
      try {
        const res = await updateShare(l.token, LIFE_SECS[life], MAX_DL[once]);
        if (!res) return;
        const list = await listShares().catch(() => [] as ShareEntry[]);
        setEntry((prev) => list.find((e) => e.token === l.token) ?? prev);
      } catch {
        /* keep the local selection; the next reopen re-syncs from list_shares */
      }
    })();
  };

  const stop = () => {
    if (link) void stopShare(link.token).catch(() => {});
    close();
    showToast(t("share.stoppedToast"));
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc
    // biome-ignore lint/a11y/useKeyWithClickEvents: same
    <div
      className="scrim"
      style={{ zIndex: 55 }}
      onMouseDown={(e) => {
        scrimDown.current = e.target === e.currentTarget;
      }}
      onClick={() => {
        if (scrimDown.current) close();
      }}
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: click-containment layer — the onClick only stops scrim-dismiss bubbling, it is not an actionable control */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
      <div
        className="modal"
        style={{ position: "relative", width: 434, fontFamily: "var(--font)" }}
        onClick={(e) => e.stopPropagation()}
      >
        {menu !== null && (
          // biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc
          // biome-ignore lint/a11y/useKeyWithClickEvents: same
          <div
            onClick={() => setMenu(null)}
            style={{ position: "absolute", inset: 0, zIndex: 5 }}
          />
        )}
        <ModalHead title={t("share.title")} sub={sub} onClose={close} />

        {noPaths || startError ? (
          <div
            style={{
              fontSize: 11.5,
              color: startError ? "var(--danger)" : "var(--muted)",
              lineHeight: 1.6,
              padding: "14px 20px 4px",
            }}
          >
            {startError ? t("share.startFailed") : t("share.demoNote")}
          </div>
        ) : (
          <>
            <div
              style={{
                display: "flex",
                gap: 14,
                alignItems: "center",
                margin: "14px 20px 0",
              }}
            >
              <div
                style={{
                  flex: 1,
                  minWidth: 0,
                  background: "var(--sidebar)",
                  borderRadius: 10,
                  padding: "12px 13px",
                }}
              >
                <div
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 13,
                    fontWeight: 600,
                    color: link ? "var(--accent-ink)" : "var(--muted)",
                    wordBreak: "break-all",
                    userSelect: "text",
                    minHeight: 18,
                  }}
                >
                  {link ? link.url : t("share.generating")}
                </div>
                {link && (
                  <div style={{ display: "flex", gap: 12, marginTop: 9 }}>
                    {/* biome-ignore lint/a11y/useSemanticElements: styled inline copy link, not a native button — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                    <span
                      role="button"
                      tabIndex={0}
                      onClick={copyLink}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          copyLink();
                        }
                      }}
                      style={{
                        fontSize: 11,
                        color: "var(--accent-ink)",
                        cursor: "pointer",
                      }}
                    >
                      {t("share.copyLink")}
                    </span>
                  </div>
                )}
              </div>
              {/* Real QR of the share URL so a phone can scan straight to the
                  browser-receive link. Shows a white placeholder of the same
                  footprint until the link exists (keeps the row from jumping). */}
              <Qr value={link ? link.url : ""} size={84} radius={9} />
            </div>
            {link && (
              <div
                style={{
                  margin: "10px 20px 0",
                  padding: "9px 11px",
                  background: "var(--sidebar)",
                  borderRadius: 9,
                  fontSize: 11,
                  lineHeight: 1.65,
                  color: "var(--muted2)",
                }}
              >
                {t("share.lanNote")}
              </div>
            )}
            {link && (
              <div
                style={{
                  margin: "8px 20px 0",
                  fontSize: 11.5,
                  color: "var(--muted)",
                  fontVariantNumeric: "tabular-nums",
                }}
              >
                {t("share.dlCount", { n: entry?.downloads ?? 0 })}
              </div>
            )}
            {link && (
              <div
                style={{ display: "flex", gap: 7, margin: "12px 20px 16px" }}
              >
                <DropPill
                  text={t("share.life", { t: lifeOpts[lifeIdx] })}
                  open={menu === "life"}
                  onToggle={() => setMenu(menu === "life" ? null : "life")}
                  minWidth={118}
                  options={lifeOpts}
                  activeIdx={lifeIdx}
                  onPick={(i) => {
                    setLifeIdx(i);
                    setMenu(null);
                    applyConfig(i, onceIdx);
                  }}
                />
                <DropPill
                  text={onceOpts[onceIdx]}
                  open={menu === "once"}
                  onToggle={() => setMenu(menu === "once" ? null : "once")}
                  minWidth={132}
                  options={onceOpts}
                  activeIdx={onceIdx}
                  onPick={(i) => {
                    setOnceIdx(i);
                    setMenu(null);
                    applyConfig(lifeIdx, i);
                  }}
                />
                <div style={{ flex: 1 }} />
                <button
                  type="button"
                  className="btn sm danger"
                  style={{ height: 26, padding: "0 12px", fontSize: 11 }}
                  onClick={stop}
                >
                  {t("share.stop")}
                </button>
              </div>
            )}
          </>
        )}
        <div className="modal-foot">{t("share.foot")}</div>
      </div>
    </div>
  );
}
