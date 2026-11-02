import { type CSSProperties, useState } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import { DirBadge, ExtChip } from "./ui";
import {
  awaitingText,
  etaClock,
  extOf,
  fmtBytes,
  fmtWhen,
} from "../lib/format";
import { resendTransfer, revealFile, transferErrText } from "../lib/sendops";
import { useEscape } from "../lib/useEscape";
import {
  sendFileFromPath,
  showToast,
  useData,
  useOverlays,
  usePrefs,
  useRecents,
  useTransfers,
} from "../lib/store";

type Ev = {
  k: string;
  tm: string;
  dot: string;
  tc: string;
  title: string;
  sub: string;
};

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div
      style={{
        flex: 1,
        background: "var(--bg)",
        borderRadius: 8,
        padding: "7px 10px",
      }}
    >
      <div style={{ fontSize: 9.5, color: "var(--muted)" }}>{label}</div>
      <div
        className="mono"
        style={{
          fontSize: 12,
          fontWeight: 600,
          color: "var(--ink2)",
          marginTop: 1,
        }}
      >
        {value}
      </div>
    </div>
  );
}

/** A transfer with more files than this opens with its folders collapsed (F5):
 *  open, a 10,000-file folder would render every one of its rows at once. */
const GROUPS_OPEN_MAX = 50;

/** One per-file row of the drawer. */
type FileRow = {
  /** manifest index — what the per-file events are keyed by */
  i: number;
  name: string;
  /** the folder the file sits in ("" for a loose file), and its own name */
  dir: string;
  base: string;
  ext: string;
  hasBar: boolean;
  barPct: number;
  label: string;
  color: string;
  done: boolean;
  failed: boolean;
};

const secBtn: CSSProperties = {
  flex: 1,
  minWidth: "calc(50% - 4px)",
  height: 30,
  borderRadius: 8,
  border: "1px solid var(--border2)",
  background: "var(--panel)",
  color: "var(--muted2)",
  fontSize: 11.5,
  fontWeight: 600,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  cursor: "pointer",
  boxSizing: "border-box",
};

/** Right-hand transfer detail drawer (opened via useOverlays.detailId). */
export default function TransferDetail() {
  const { t } = useTranslation();
  const detailId = useOverlays((s) => s.detailId);
  const setDetail = useOverlays((s) => s.setDetail);
  const openSend = useOverlays((s) => s.openSend);
  const tr = useTransfers((s) =>
    detailId ? s.transfers[detailId] : undefined,
  );
  const removeTransfer = useTransfers((s) => s.removeTransfer);
  const downloadDir = useData((s) => s.downloadDir);
  const port = usePrefs((s) => s.port);
  const recents = useRecents((s) => s.items);
  useEscape(!!tr && tr.kind !== "text" && tr.via !== "browser", () =>
    setDetail(null),
  );
  // Folders opened or closed by hand (F5) — for the transfer they were
  // toggled on, so another transfer's drawer starts from its own defaults.
  const [toggled, setToggled] = useState<{
    id: string | null;
    dirs: Record<string, boolean>;
  }>({ id: null, dirs: {} });
  if (!tr) return null;
  // Quick-text records (M7.3) and browser-share downloads (M8.4) live in the
  // history list only — they carry no session/paths, so this file-oriented
  // drawer has nothing to show. Safety net if one is ever routed here.
  if (tr.kind === "text" || tr.via === "browser") return null;

  const dir = tr.direction;
  const dirColor = dir === "receive" ? "var(--success)" : "var(--accent)";
  const speedColor = dir === "receive" ? "var(--success)" : "var(--accent-ink)";
  const isDone = tr.status === "done";
  const isError = tr.status === "error";
  const isActive = tr.status === "active";
  // Parked on the concurrency gate (M6.7): no bytes moving yet, so the status
  // line reads "waiting for a slot" instead of a misleading 0.0 MB/s.
  const isQueued = tr.status === "queued";
  // Sent, not yet accepted: the receiver's prompt is up (H10).
  const isAwaiting = dir === "send" && isActive && !tr.accepted;
  const isTerminal = isDone || isError;
  // Every code translates at render time. This used to hand-translate three of
  // them and fall back to `tr.error` for the rest — and `tr.error` is the
  // backend's English internal diagnostic, so cancelling a transfer showed the
  // peer a line of raw Rust ("protocol: peer closed connection") and an I/O
  // failure showed them a local absolute path. `transferErrText` covers the
  // whole closed set of codes and never reaches for the raw string.
  const isCanceled = tr.errorCode === "cancelled";
  const errorText = isCanceled
    ? t("transfers.statusCanceled")
    : transferErrText(tr.errorCode);
  const peer = tr.peerName ?? "";
  const firstName =
    tr.name ?? tr.files?.[0]?.name ?? tr.savedNames?.[0] ?? tr.sessionId;
  const pct = Math.min(tr.percent, 100);
  const doneBytes = (tr.totalSize * pct) / 100;
  // The count the backend gave wins: a send's `files` is the selection until
  // it starts (a folder is one entry), and a row restored from history keeps
  // only the first names (KEPT_FILE_NAMES).
  const filesN = tr.fileCount ?? tr.files?.length ?? 1;
  // What was actually verified (M13): the files whose own completion event
  // said so. It used to be this machine's 完整性校验 setting — which decides
  // what THIS machine sends; whether a receive carries checksums is the
  // sender's setting, so the drawer reported 「SHA-256 校验通过 n/n」 for files
  // nobody had checked.
  const verifiedN = Object.values(tr.fileStat ?? {}).filter(
    (f) => f.done && f.verified,
  ).length;
  const etaStr = etaClock(tr.totalSize, tr.percent, tr.speedBps);

  const hist = tr.hist.slice(-40);
  const durS = Math.max(
    1,
    Math.round(((tr.doneAt ?? Date.now()) - tr.startedAt) / 1000),
  );
  const durStr =
    durS >= 60
      ? t("detail.durMinSec", { m: Math.floor(durS / 60), s: durS % 60 })
      : t("detail.durSec", { s: durS });
  const avgMBs = hist.length
    ? hist.reduce((a, b) => a + b, 0) / hist.length
    : tr.totalSize / 1048576 / durS;
  const peakMBs = hist.length ? Math.max(...hist) : avgMBs;

  /* speed curve (max-normalized, 330×64) */
  const showCurve = hist.length > 1;
  let lineD = "";
  let areaD = "";
  if (showCurve) {
    const cw = 330;
    const ch = 64;
    const cmx = Math.max(...hist, 10);
    const step = cw / Math.max(hist.length - 1, 1);
    lineD =
      "M" +
      hist
        .map(
          (v, i) =>
            `${(i * step).toFixed(1)},${(ch - 4 - (v / cmx) * (ch - 12)).toFixed(1)}`,
        )
        .join(" L");
    areaD = `${lineD} L${cw},${ch} L0,${ch} Z`;
  }

  /* event timeline — real events only */
  const evs: Ev[] = [];
  evs.push({
    k: "created",
    tm: "0s",
    dot: "var(--muted)",
    tc: "var(--ink2)",
    title: t("detail.evCreated"),
    sub:
      dir === "send"
        ? t("detail.evCreatedOutSub")
        : t("detail.evCreatedInSub", { peer }),
  });
  if (tr.started)
    evs.push({
      k: "started",
      tm: "0s",
      dot: "var(--accent)",
      tc: "var(--ink2)",
      title: t("detail.evStarted"),
      sub: t("detail.evStartedSub", { port }),
    });
  if (isDone) {
    evs.push({
      k: "done",
      tm: `${durS}s`,
      dot: "var(--success)",
      tc: "var(--ink2)",
      title: t("detail.evDone"),
      sub: t("detail.evDoneSub", {
        v: Math.round(peakMBs),
        when: tr.doneAt ? fmtWhen(tr.doneAt) : t("transfers.justNow"),
      }),
    });
    if (verifiedN > 0)
      evs.push({
        k: "verified",
        tm: `${durS}s`,
        dot: "var(--success)",
        tc: "var(--success)",
        title: t("detail.evVerified"),
        sub: t("detail.evVerifiedSub", { n: verifiedN, total: filesN }),
      });
  } else if (isError) {
    evs.push({
      k: "ended",
      tm: `${durS}s`,
      dot: isCanceled ? "var(--muted)" : "var(--danger)",
      tc: isCanceled ? "var(--muted2)" : "var(--danger)",
      title: isCanceled ? t("detail.evCanceled") : t("detail.evFailed"),
      sub: isCanceled
        ? t("detail.evCanceledSub")
        : t("detail.evFailedSub", { reason: errorText ?? "" }),
    });
  } else {
    const etaSeg = etaStr ? t("transfers.eta", { t: etaStr }) : "";
    evs.push({
      k: "current",
      tm: t("detail.evNow"),
      dot: dirColor,
      tc: "var(--ink2)",
      title: tr.started
        ? t("detail.evActive", { speed: (tr.speedBps / 1048576).toFixed(1) })
        : t("detail.evWaitingAccept"),
      sub: etaSeg
        ? t("detail.evActiveSub", { pct: Math.round(pct), eta: etaSeg })
        : t("detail.evActivePctOnly", { pct: Math.round(pct) }),
    });
  }

  /* per-file rows — real transfer_file_* events (M6.8) win per file, keyed by
     manifest index; files with no event yet fall back to the cumulative-size
     estimate (also the whole path for a session that predates per-file events) */
  const fileList: { name: string; size?: number }[] = tr.files?.length
    ? tr.files
    : tr.savedNames?.length
      ? tr.savedNames.map((name) => ({ name }))
      : [{ name: firstName }];
  const sizesKnown = fileList.every((f) => typeof f.size === "number");
  const equalShare = tr.totalSize / Math.max(fileList.length, 1);
  let acc = 0;
  const fileRows: FileRow[] = fileList.map((f, i) => {
    const stat = tr.fileStat?.[i];
    const fsize = sizesKnown ? (f.size as number) : equalShare;
    const start = acc;
    acc += fsize;
    let hasBar = false;
    let barPct = 0;
    let label = t("detail.fileQueued");
    let color = "var(--muted)";
    let done = false;
    let failed = false;
    if (stat?.done) {
      // A real transfer_file_done arrived: ✓ (verified when the hash matched).
      label = stat.verified ? t("detail.fileVerified") : t("detail.fileDone");
      color = "var(--success)";
      done = true;
    } else if (stat) {
      // A real transfer_file_progress arrived — show its exact percent.
      hasBar = true;
      barPct = Math.min(100, Math.max(0, Math.round(stat.percent)));
    } else if (isError) {
      label = isCanceled ? t("detail.fileCanceled") : t("detail.fileFailed");
      color = isCanceled ? "var(--muted)" : "var(--danger)";
      failed = !isCanceled;
    } else if (isDone || doneBytes >= acc - 1) {
      // No event for this file, so nothing says it was verified.
      label = t("detail.fileDone");
      color = "var(--success)";
      done = true;
    } else if (doneBytes > start) {
      hasBar = true;
      barPct = Math.round(((doneBytes - start) / Math.max(fsize, 1)) * 100);
    }
    const slash = f.name.lastIndexOf("/");
    const base = f.name.slice(slash + 1);
    return {
      i,
      name: f.name,
      dir: slash > 0 ? f.name.slice(0, slash) : "",
      base,
      ext: extOf(base),
      hasBar,
      barPct,
      label,
      color,
      done,
      failed,
    };
  });
  // Names a restored row no longer has (KEPT_FILE_NAMES): counted, not listed.
  const unlisted =
    tr.files?.length && tr.fileCount ? tr.fileCount - tr.files.length : 0;

  /* by folder (F5): the files of a folder send sit under the folder they are
     in, in the order the folders first come up; loose files get no header. */
  const groups: { dir: string; rows: FileRow[] }[] = [];
  const byDir = new Map<string, { dir: string; rows: FileRow[] }>();
  for (const r of fileRows) {
    let g = byDir.get(r.dir);
    if (!g) {
      g = { dir: r.dir, rows: [] };
      byDir.set(r.dir, g);
      groups.push(g);
    }
    g.rows.push(r);
  }
  const grouped = groups.some((g) => g.dir !== "");
  const dirsToggled = toggled.id === tr.sessionId ? toggled.dirs : {};
  const isOpen = (d: string) =>
    dirsToggled[d] ?? fileRows.length <= GROUPS_OPEN_MAX;
  const toggleDir = (d: string) =>
    setToggled({
      id: tr.sessionId,
      dirs: { ...dirsToggled, [d]: !isOpen(d) },
    });

  /* one file's row; under an open folder it is indented and goes by its own name */
  const fileRow = (r: FileRow, indented: boolean) => (
    <div
      key={r.i}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 9,
        padding: indented ? "9px 12px 9px 34px" : "9px 12px",
        borderBottom: "1px solid var(--border)",
      }}
    >
      <ExtChip ext={r.ext} size={26} fontSize={7.5} radius={7} />
      <span
        style={{
          flex: 1,
          minWidth: 0,
          fontSize: 11.5,
          color: "var(--ink2)",
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
        }}
      >
        {indented ? r.base : r.name}
      </span>
      {r.hasBar ? (
        <>
          <span
            style={{
              width: 52,
              height: 4,
              borderRadius: 99,
              background: "var(--track)",
              overflow: "hidden",
              flex: "none",
            }}
          >
            <span
              style={{
                display: "block",
                width: `${r.barPct}%`,
                height: "100%",
                background: dirColor,
              }}
            />
          </span>
          <span
            className="mono"
            style={{
              fontSize: 10,
              color: "var(--muted2)",
              flex: "none",
            }}
          >
            {r.barPct}%
          </span>
        </>
      ) : (
        <span style={{ fontSize: 10.5, color: r.color, flex: "none" }}>
          {r.label}
        </span>
      )}
    </div>
  );
  const folderRow = (g: { dir: string; rows: FileRow[] }) => {
    const open = isOpen(g.dir);
    const doneN = g.rows.filter((r) => r.done).length;
    const countColor = g.rows.some((r) => r.failed)
      ? "var(--danger)"
      : doneN === g.rows.length
        ? "var(--success)"
        : "var(--muted2)";
    return (
      <button
        key={`dir:${g.dir}`}
        type="button"
        aria-expanded={open}
        onClick={() => toggleDir(g.dir)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          width: "100%",
          padding: "8px 12px",
          border: 0,
          borderBottom: "1px solid var(--border)",
          background: "var(--bg)",
          color: "var(--ink2)",
          font: "inherit",
          textAlign: "left",
          cursor: "pointer",
        }}
      >
        <svg
          width="10"
          height="10"
          viewBox="0 0 10 10"
          aria-hidden="true"
          style={{
            flex: "none",
            color: "var(--muted)",
            transform: open ? "rotate(90deg)" : "none",
            transition: "transform .15s",
          }}
        >
          <path
            d="M3 1.5 6.5 5 3 8.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          />
        </svg>
        <svg
          width="16"
          height="16"
          viewBox="0 0 16 16"
          aria-hidden="true"
          style={{ flex: "none", color: "var(--muted2)" }}
        >
          <path
            d="M1.5 4.25c0-.69.56-1.25 1.25-1.25h3.3l1.5 1.6h5.7c.69 0 1.25.56 1.25 1.25v6.4c0 .69-.56 1.25-1.25 1.25H2.75c-.69 0-1.25-.56-1.25-1.25z"
            fill="currentColor"
            opacity=".85"
          />
        </svg>
        <span
          style={{
            flex: 1,
            minWidth: 0,
            fontSize: 11.5,
            fontWeight: 600,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {g.dir}
        </span>
        <span
          className="mono"
          style={{ fontSize: 10, color: countColor, flex: "none" }}
        >
          {doneN}/{g.rows.length}
        </span>
      </button>
    );
  };

  /* actions */
  const canResend = dir === "send" && !!tr.paths?.length;
  const doResend = () => {
    if (resendTransfer(tr)) setDetail(null);
  };
  const doOpenDir = () => {
    void api
      .revealReceived(tr.sessionId)
      .then((paths) =>
        paths[0]
          ? revealFile(paths[0])
          : showToast(t("detail.openDirUnavailable")),
      )
      .catch(() => showToast(t("detail.openDirUnavailable")));
  };
  const doSendOther = () => {
    setDetail(null);
    openSend(null, recents, (tr.paths ?? []).map(sendFileFromPath));
  };
  const doRemove = () => {
    removeTransfer(tr.sessionId);
    setDetail(null);
    showToast(t("detail.removedToast"));
  };
  const primary = isTerminal
    ? canResend
      ? { label: t("detail.primaryResend"), fn: doResend }
      : dir === "receive" && isDone
        ? { label: t("detail.primaryOpenDir"), fn: doOpenDir }
        : null
    : null;

  const footLeft =
    dir === "receive" && isDone
      ? verifiedN > 0
        ? t("detail.footVerified", { n: verifiedN, total: filesN })
        : t("detail.footNotVerified")
      : t("app.name");
  const footRight = dir === "receive" ? downloadDir : t("detail.footSendDir");

  return (
    <>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
      <div
        onClick={() => setDetail(null)}
        style={{ position: "fixed", inset: 0, zIndex: 45 }}
      />
      <div
        style={{
          position: "fixed",
          top: 0,
          right: 0,
          bottom: 0,
          width: 392,
          zIndex: 46,
          background: "var(--panel)",
          borderLeft: "1px solid var(--border)",
          boxShadow: "var(--shadow)",
          display: "flex",
          flexDirection: "column",
          animation: "lbDrawer .2s ease",
          color: "var(--ink)",
        }}
      >
        {/* header */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            padding: "13px 18px",
            borderBottom: "1px solid var(--border)",
            flex: "none",
          }}
        >
          <DirBadge dir={dir} />
          <span
            style={{
              fontSize: 12,
              color: "var(--muted2)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {dir === "receive"
              ? t("detail.from", { peer })
              : t("detail.to", { peer })}
          </span>
          <button
            type="button"
            aria-label={t("common.close")}
            title={t("common.close")}
            onClick={() => setDetail(null)}
            style={{
              marginLeft: "auto",
              width: 26,
              height: 26,
              borderRadius: 7,
              border: "none",
              background: "none",
              color: "var(--muted)",
              fontSize: 14,
              cursor: "pointer",
              fontFamily: "inherit",
              flex: "none",
            }}
            onMouseEnter={(e) =>
              (e.currentTarget.style.background = "var(--hover)")
            }
            onMouseLeave={(e) => (e.currentTarget.style.background = "none")}
          >
            ×
          </button>
        </div>

        {/* title block */}
        <div style={{ padding: "14px 18px 0", flex: "none" }}>
          <div style={{ fontSize: 14, fontWeight: 650, color: "var(--ink2)" }}>
            {firstName}
          </div>
          <div style={{ fontSize: 11, color: "var(--muted)", marginTop: 2 }}>
            {t("detail.sizeLine", {
              count: filesN,
              size: fmtBytes(tr.totalSize),
            })}
          </div>
          {!isDone && (
            <>
              <div
                style={{
                  position: "relative",
                  height: 6,
                  borderRadius: 99,
                  background: "var(--track)",
                  marginTop: 10,
                  overflow: "hidden",
                }}
              >
                <div
                  style={{
                    width: `${pct}%`,
                    height: "100%",
                    borderRadius: 99,
                    background: isError ? "var(--danger)" : dirColor,
                  }}
                />
              </div>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  marginTop: 6,
                }}
              >
                <span
                  className="mono"
                  style={{
                    fontSize: 11,
                    color: isError ? "var(--danger)" : speedColor,
                    // A waiting send's line carries the SAS both screens
                    // compare: it wraps rather than lose the code to "…".
                    whiteSpace: isAwaiting ? "normal" : "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                  }}
                >
                  {isError
                    ? (errorText ?? t("transfers.statusFailed"))
                    : isQueued
                      ? t("transfers.queued")
                      : isAwaiting
                        ? awaitingText(tr)
                        : `${(tr.speedBps / 1048576).toFixed(1)} MB/s`}
                </span>
                <span
                  style={{
                    fontSize: 10.5,
                    color: "var(--muted)",
                    flex: "none",
                  }}
                >
                  {fmtBytes(doneBytes)} / {fmtBytes(tr.totalSize)} ·{" "}
                  {Math.round(pct)}%
                  {isActive && etaStr
                    ? ` · ${t("transfers.eta", { t: etaStr })}`
                    : ""}
                </span>
              </div>
            </>
          )}
          {isDone && (
            <div style={{ display: "flex", gap: 8, marginTop: 11 }}>
              <Stat label={t("detail.duration")} value={durStr} />
              <Stat
                label={t("detail.avgSpeed")}
                value={`${avgMBs.toFixed(1)} MB/s`}
              />
              <Stat label={t("detail.resume")} value={t("detail.resumeNone")} />
            </div>
          )}
        </div>

        {/* scrolling middle */}
        <div className="scroll-y" style={{ flex: 1 }}>
          <div style={{ padding: "0 18px 14px" }}>
            {showCurve && (
              <div
                style={{
                  marginTop: 13,
                  background: "var(--bg)",
                  border: "1px solid var(--border)",
                  borderRadius: 10,
                  padding: "10px 12px 8px",
                }}
              >
                <div
                  style={{
                    display: "flex",
                    alignItems: "baseline",
                    justifyContent: "space-between",
                    gap: 10,
                  }}
                >
                  <span
                    style={{
                      fontSize: 10.5,
                      fontWeight: 600,
                      color: "var(--muted2)",
                    }}
                  >
                    {isActive
                      ? t("detail.curveLive")
                      : t("detail.curveAll", { t: durStr })}
                  </span>
                  <span
                    className="mono"
                    style={{ fontSize: 10, color: "var(--muted)" }}
                  >
                    {t("detail.peak", { v: Math.round(peakMBs) })}
                  </span>
                </div>
                <div style={{ position: "relative", marginTop: 7 }}>
                  <svg
                    width="100%"
                    height={64}
                    viewBox="0 0 330 64"
                    preserveAspectRatio="none"
                    style={{ display: "block" }}
                  >
                    <title>
                      {isActive
                        ? t("detail.curveLive")
                        : t("detail.curveAll", { t: durStr })}
                    </title>
                    <path
                      d={areaD}
                      fill={
                        dir === "receive"
                          ? "var(--success-soft)"
                          : "var(--accent-soft)"
                      }
                    />
                    <path
                      d={lineD}
                      fill="none"
                      stroke={isError ? "var(--border2)" : dirColor}
                      strokeWidth={1.5}
                    />
                  </svg>
                </div>
                <div
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    marginTop: 4,
                  }}
                >
                  <span
                    className="mono"
                    style={{ fontSize: 9, color: "var(--muted)" }}
                  >
                    0s
                  </span>
                  <span
                    className="mono"
                    style={{ fontSize: 9, color: "var(--muted)" }}
                  >
                    {isActive
                      ? t("detail.axisNow")
                      : t("detail.axisAbout", { t: `${durS}s` })}
                  </span>
                </div>
              </div>
            )}

            {/* event timeline */}
            <div style={{ padding: "14px 2px 2px" }}>
              {evs.map((ev, i) => (
                <div key={ev.k} style={{ display: "flex", gap: 10 }}>
                  <span
                    className="mono"
                    style={{
                      fontSize: 9.5,
                      color: "var(--muted)",
                      width: 36,
                      textAlign: "right",
                      flex: "none",
                      paddingTop: 1,
                    }}
                  >
                    {ev.tm}
                  </span>
                  <div
                    style={{
                      width: 7,
                      flex: "none",
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "center",
                    }}
                  >
                    <span
                      style={{
                        width: 7,
                        height: 7,
                        borderRadius: "50%",
                        background: ev.dot,
                        flex: "none",
                        marginTop: 3,
                      }}
                    />
                    <span
                      style={{
                        flex: 1,
                        width: 1,
                        background:
                          i === evs.length - 1
                            ? "transparent"
                            : "var(--border)",
                      }}
                    />
                  </div>
                  <div
                    style={{
                      flex: 1,
                      minWidth: 0,
                      paddingBottom: i === evs.length - 1 ? 4 : 13,
                    }}
                  >
                    <div
                      style={{ fontSize: 11.5, fontWeight: 600, color: ev.tc }}
                    >
                      {ev.title}
                    </div>
                    <div
                      style={{
                        fontSize: 10.5,
                        color: "var(--muted)",
                        marginTop: 1,
                      }}
                    >
                      {ev.sub}
                    </div>
                  </div>
                </div>
              ))}
            </div>

            {/* per-file rows */}
            <div
              style={{
                border: "1px solid var(--border)",
                borderRadius: 10,
                overflow: "hidden",
              }}
            >
              {grouped
                ? groups.flatMap((g) =>
                    g.dir === ""
                      ? g.rows.map((r) => fileRow(r, false))
                      : [
                          folderRow(g),
                          ...(isOpen(g.dir)
                            ? g.rows.map((r) => fileRow(r, true))
                            : []),
                        ],
                  )
                : fileRows.map((r) => fileRow(r, false))}
              {unlisted > 0 && (
                <div
                  style={{
                    padding: "9px 12px",
                    fontSize: 11,
                    color: "var(--muted)",
                  }}
                >
                  {t("detail.moreFiles", { count: unlisted })}
                </div>
              )}
            </div>

            {/* actions (terminal only) */}
            {isTerminal && (
              <div style={{ marginTop: 13 }}>
                {primary && (
                  // biome-ignore lint/a11y/useSemanticElements: styled action button, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown
                  <div
                    role="button"
                    tabIndex={0}
                    onClick={primary.fn}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        if (e.key === " ") e.preventDefault();
                        primary.fn();
                      }
                    }}
                    style={{
                      height: 32,
                      borderRadius: 8,
                      background: "var(--accent)",
                      color: "var(--accent-fg)",
                      fontSize: 12,
                      fontWeight: 600,
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      cursor: "pointer",
                    }}
                    onMouseEnter={(e) =>
                      (e.currentTarget.style.filter = "brightness(.94)")
                    }
                    onMouseLeave={(e) => (e.currentTarget.style.filter = "")}
                  >
                    {primary.label}
                  </div>
                )}
                <div
                  style={{
                    display: "flex",
                    flexWrap: "wrap",
                    gap: 8,
                    marginTop: 8,
                  }}
                >
                  {canResend && (
                    // biome-ignore lint/a11y/useSemanticElements: styled action button, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown
                    <div
                      role="button"
                      tabIndex={0}
                      onClick={doSendOther}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          if (e.key === " ") e.preventDefault();
                          doSendOther();
                        }
                      }}
                      style={secBtn}
                      onMouseEnter={(e) =>
                        (e.currentTarget.style.background = "var(--hover)")
                      }
                      onMouseLeave={(e) =>
                        (e.currentTarget.style.background = "var(--panel)")
                      }
                    >
                      {t("detail.sendOther")}
                    </div>
                  )}
                  {/* biome-ignore lint/a11y/useSemanticElements: styled action button, not a native <button> — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                  <div
                    role="button"
                    tabIndex={0}
                    onClick={doRemove}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        if (e.key === " ") e.preventDefault();
                        doRemove();
                      }
                    }}
                    style={{ ...secBtn, color: "var(--danger)" }}
                    onMouseEnter={(e) =>
                      (e.currentTarget.style.borderColor = "var(--danger)")
                    }
                    onMouseLeave={(e) =>
                      (e.currentTarget.style.borderColor = "var(--border2)")
                    }
                  >
                    {t("detail.removeRecord")}
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>

        {/* footer strip */}
        <div
          style={{
            flex: "none",
            display: "flex",
            justifyContent: "space-between",
            gap: 10,
            background: "var(--sidebar)",
            padding: "10px 18px",
            fontSize: 10.5,
            color: "var(--muted2)",
          }}
        >
          <span style={{ whiteSpace: "nowrap" }}>{footLeft}</span>
          <span
            className="mono"
            style={{
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {footRight}
          </span>
        </div>
      </div>
    </>
  );
}
