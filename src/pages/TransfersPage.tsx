import { memo, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import { DirBadge, ExtChip, Segmented } from "../components/ui";
import { etaClock, extOf, fmtBytes, fmtWhen } from "../lib/format";
import { copyText, resendTransfer } from "../lib/sendops";
import {
  showToast,
  transferList,
  useOverlays,
  useTransfers,
  type UITransfer,
} from "../lib/store";

type Filter = "all" | "active" | "done";

const cardHead: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "13px 16px",
  borderBottom: "1px solid var(--border)",
};

const emptyNote: CSSProperties = {
  padding: 26,
  textAlign: "center",
  fontSize: 12,
  color: "var(--muted)",
};

/** First file name (+ 等 n 个文件) shown as the row title. */
function rowName(tr: UITransfer, t: TFunction): string {
  const first = tr.name ?? tr.files?.[0]?.name ?? tr.savedNames?.[0] ?? "";
  const n = tr.fileCount ?? tr.files?.length ?? 1;
  return n > 1 ? t("transfers.filesMore", { name: first, n }) : first;
}

function rowExt(tr: UITransfer): string {
  return (
    tr.ext ?? extOf(tr.name ?? tr.files?.[0]?.name ?? tr.savedNames?.[0] ?? "")
  );
}

function RunningRow({ tr }: { tr: UITransfer }) {
  const { t } = useTranslation();
  const setDetail = useOverlays((s) => s.setDetail);
  const setPaused = useTransfers((s) => s.setPaused);
  const dirColor =
    tr.direction === "receive" ? "var(--success)" : "var(--accent)";
  const speedColor =
    tr.direction === "receive" ? "var(--success)" : "var(--accent-ink)";
  // Parked on the concurrency gate (M6.7): no bytes are moving yet, so the
  // speed/ETA and the pause control are meaningless — swap in a waiting hint.
  const queued = tr.status === "queued";
  const pct = Math.min(tr.percent, 100);
  const doneBytes = (tr.totalSize * pct) / 100;
  const eta = etaClock(tr.totalSize, tr.percent, tr.speedBps);
  const hist = tr.hist.slice(-40);
  const mx = Math.max(20, ...hist);

  // Pause is session-local backpressure (M6.2) — flip the UI flag optimistically
  // (there is no backend "paused" event) and stall/resume the byte loop.
  const togglePause = (e: ReactMouseEvent<HTMLElement>) => {
    e.stopPropagation();
    if (tr.paused) {
      void api.resumeTransfer(tr.sessionId);
      setPaused(tr.sessionId, false);
    } else {
      void api.pauseTransfer(tr.sessionId);
      setPaused(tr.sessionId, true);
    }
  };
  // Cancel drops the session (M6.1); the peer fails through its error path and
  // the backend emits transfer_error{code:"cancelled"}, moving the row to history.
  const cancel = (e: ReactMouseEvent<HTMLElement>) => {
    e.stopPropagation();
    void api.cancelTransfer(tr.sessionId);
    showToast(t("transfers.canceledToast"));
  };
  const tick = (at: number): CSSProperties => ({
    position: "absolute",
    left: `${at}%`,
    top: 0,
    bottom: 0,
    width: 1,
    background: pct > at ? "rgba(255,255,255,.55)" : "var(--border2)",
  });

  return (
    // biome-ignore lint/a11y/useSemanticElements: styled progress row kept — a <button> would change the row's layout/styling; made keyboard-operable via role/tabIndex/onKeyDown
    <div
      className="hover-row"
      role="button"
      tabIndex={0}
      onClick={() => setDetail(tr.sessionId)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          if (e.key === " ") e.preventDefault();
          setDetail(tr.sessionId);
        }
      }}
      style={{
        padding: "13px 16px",
        borderBottom: "1px solid var(--border)",
        cursor: "pointer",
        transition: "background .15s ease",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <ExtChip ext={rowExt(tr)} size={32} fontSize={9} radius={8} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <span
            style={{ fontSize: 12.5, fontWeight: 600, color: "var(--ink2)" }}
          >
            {rowName(tr, t)}
          </span>
          <DirBadge
            dir={tr.direction}
            style={{ marginLeft: 8, verticalAlign: 1 }}
          />
          <span style={{ fontSize: 11, color: "var(--muted)", marginLeft: 6 }}>
            {tr.peerName ?? ""} · {fmtBytes(tr.totalSize)}
          </span>
        </div>
        <div style={{ textAlign: "right", flex: "none" }}>
          {queued ? (
            <div
              style={{
                fontSize: 11,
                color: "var(--muted2)",
                whiteSpace: "nowrap",
              }}
            >
              {t("transfers.queued")}
            </div>
          ) : (
            <>
              <div
                className="mono"
                style={{ fontSize: 11.5, color: speedColor }}
              >
                {(tr.speedBps / 1048576).toFixed(1)} MB/s
              </div>
              <div style={{ fontSize: 10.5, color: "var(--muted)" }}>
                {eta ? t("transfers.eta", { t: eta }) : ""}
              </div>
            </>
          )}
        </div>
        <div
          className="mono"
          style={{
            fontSize: 11.5,
            fontWeight: 600,
            color: "var(--ink2)",
            width: 44,
            textAlign: "right",
            flex: "none",
          }}
        >
          {Math.round(pct)}%
        </div>
        <div style={{ display: "flex", gap: 4, flex: "none" }}>
          {!queued && (
            <button
              type="button"
              onClick={togglePause}
              style={{
                padding: "4px 10px",
                borderRadius: 7,
                fontSize: 11.5,
                color: "var(--muted2)",
                border: "1px solid var(--border2)",
                background: "var(--panel)",
                cursor: "pointer",
                fontFamily: "inherit",
                whiteSpace: "nowrap",
              }}
              onMouseEnter={(e) =>
                (e.currentTarget.style.background = "var(--hover)")
              }
              onMouseLeave={(e) =>
                (e.currentTarget.style.background = "var(--panel)")
              }
            >
              {tr.paused ? t("transfers.resume") : t("transfers.pause")}
            </button>
          )}
          <button
            type="button"
            onClick={cancel}
            style={{
              padding: "4px 10px",
              borderRadius: 7,
              fontSize: 11.5,
              color: "var(--muted)",
              border: "none",
              background: "none",
              cursor: "pointer",
              fontFamily: "inherit",
              whiteSpace: "nowrap",
            }}
            onMouseEnter={(e) =>
              (e.currentTarget.style.color = "var(--danger)")
            }
            onMouseLeave={(e) => (e.currentTarget.style.color = "var(--muted)")}
          >
            {t("transfers.cancel")}
          </button>
        </div>
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "flex-end",
          gap: 14,
          margin: "9px 0 0 44px",
        }}
      >
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              position: "relative",
              height: 5,
              borderRadius: 99,
              background: "var(--track)",
              overflow: "hidden",
            }}
          >
            <div
              style={{
                width: `${pct}%`,
                height: "100%",
                borderRadius: 99,
                background: dirColor,
              }}
            />
            <div style={tick(25)} />
            <div style={tick(50)} />
            <div style={tick(75)} />
          </div>
          <div
            className="mono"
            style={{ fontSize: 10.5, color: "var(--muted)", marginTop: 6 }}
          >
            {t("transfers.metaLine", {
              done: fmtBytes(doneBytes),
              total: fmtBytes(tr.totalSize),
              pct: Math.round(pct),
            })}
            {" · "}
            {queued
              ? t("transfers.queued")
              : tr.paused
                ? t("transfers.speedPaused")
                : t("transfers.shaChecking")}
          </div>
        </div>
        {hist.length > 0 && (
          <div
            style={{
              display: "flex",
              alignItems: "flex-end",
              gap: 1,
              height: 30,
              width: 162,
              flex: "none",
              borderBottom: "1px solid var(--border2)",
              paddingBottom: 1,
              overflow: "hidden",
            }}
          >
            {hist.map((v, i) => (
              <div
                key={i}
                style={{
                  width: 3,
                  height: 2 + (v / mx) * 26,
                  background: dirColor,
                  opacity: i === hist.length - 1 ? 1 : 0.85,
                  borderRadius: "1px 1px 0 0",
                  flex: "none",
                }}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// Memoized: `tr` keeps referential identity across a transfer's progress ticks
// (the store spreads only the active session), so terminal history rows skip the
// ~10-20 re-renders/s a live transfer would otherwise inflict. The device list is
// read lazily via getState() at click time — subscribing here would re-render
// every memoized row on LAN-discovery churn, defeating the memo.
const HistoryRow = memo(function HistoryRow({ tr }: { tr: UITransfer }) {
  const { t } = useTranslation();
  const setDetail = useOverlays((s) => s.setDetail);
  const isDone = tr.status === "done";
  // A user-cancelled transfer (M6.1) is an outcome, not a failure — show 已取消
  // in a muted tone rather than the red 失败.
  const isCanceled = tr.errorCode === "cancelled";
  const canResend =
    tr.status === "error" && tr.direction === "send" && !!tr.paths?.length;
  // A browser-share download (M8.4): a done "send" row that opens no drawer —
  // there is nothing to show (no session, no source paths). It renders
  // non-interactive below, so `open` only ever fires for a real transfer.
  const isBrowser = tr.via === "browser";
  const open = () => setDetail(tr.sessionId);

  const resend = (e: ReactMouseEvent<HTMLElement>) => {
    e.stopPropagation();
    resendTransfer(tr);
  };

  const inner = (
    <>
      <ExtChip ext={rowExt(tr)} size={32} fontSize={9} radius={8} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <span style={{ fontSize: 12.5, fontWeight: 600, color: "var(--ink2)" }}>
          {rowName(tr, t)}
        </span>
        <DirBadge
          dir={tr.direction}
          style={{ marginLeft: 8, verticalAlign: 1 }}
        />
        <span style={{ fontSize: 11, color: "var(--muted)", marginLeft: 6 }}>
          {tr.peerName ?? ""}
        </span>
        {isBrowser && (
          <span
            style={{
              fontSize: 10,
              color: "var(--muted2)",
              marginLeft: 6,
              padding: "1px 6px",
              borderRadius: 5,
              background: "var(--sidebar)",
            }}
          >
            {t("transfers.viaBrowser")}
          </span>
        )}
      </div>
      <span
        className="mono"
        style={{ fontSize: 11, color: "var(--muted)", flex: "none" }}
      >
        {fmtBytes(tr.totalSize)}
      </span>
      {canResend && (
        // biome-ignore lint/a11y/useSemanticElements: styled inline action kept — a <button> would change the chip's styling; made keyboard-operable via role/tabIndex/onKeyDown
        <span
          role="button"
          tabIndex={0}
          onClick={resend}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              if (e.key === " ") e.preventDefault();
              e.stopPropagation();
              resendTransfer(tr);
            }
          }}
          style={{
            fontSize: 11,
            color: "var(--accent-ink)",
            cursor: "pointer",
            flex: "none",
          }}
        >
          {t("transfers.resend")}
        </span>
      )}
      <span
        style={{
          fontSize: 11.5,
          fontWeight: 600,
          color: isDone
            ? "var(--success)"
            : isCanceled
              ? "var(--muted)"
              : "var(--danger)",
          flex: "none",
          width: 52,
          textAlign: "right",
        }}
      >
        {isDone
          ? t("transfers.statusDone")
          : isCanceled
            ? t("transfers.statusCanceled")
            : t("transfers.statusFailed")}
      </span>
      <span
        style={{
          fontSize: 11,
          color: "var(--muted)",
          flex: "none",
          width: 70,
          textAlign: "right",
        }}
      >
        {tr.doneAt ? fmtWhen(tr.doneAt) : ""}
      </span>
    </>
  );

  const rowStyle = {
    display: "flex",
    alignItems: "center",
    gap: 12,
    padding: "12px 16px",
    borderBottom: "1px solid var(--border)",
    transition: "background .15s ease",
  } as const;

  // A browser-download row has nothing to open — render it NON-interactive (no
  // role/tabIndex) rather than a focusable button that does nothing.
  if (isBrowser) {
    return (
      <div className="hover-row" style={{ ...rowStyle, cursor: "default" }}>
        {inner}
      </div>
    );
  }

  return (
    // biome-ignore lint/a11y/useSemanticElements: styled history row kept — a <button> would change the row's layout/styling; made keyboard-operable via role/tabIndex/onKeyDown
    <div
      className="hover-row"
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          if (e.key === " ") e.preventDefault();
          open();
        }
      }}
      style={{ ...rowStyle, cursor: "pointer" }}
    >
      {inner}
    </div>
  );
});

/** A quick-text (M7.3) history entry: a stripped-down row — txt chip, the text
 *  preview, a direction pill, the peer and the time. No size / speed / progress
 *  / status column; clicking (or the Copy chip) copies the text to clipboard.
 *  Memoized like HistoryRow: `tr` is referentially stable once terminal, so the
 *  row is inert during live-transfer progress ticks (it has no store subscriptions). */
const TextRow = memo(function TextRow({ tr }: { tr: UITransfer }) {
  const { t } = useTranslation();
  const copy = () => {
    copyText(tr.text ?? "");
    showToast(t("inbox.copiedText"));
  };
  return (
    // biome-ignore lint/a11y/useSemanticElements: styled text row kept — a <button> would change the row's layout/styling; made keyboard-operable via role/tabIndex/onKeyDown
    <div
      className="hover-row"
      role="button"
      tabIndex={0}
      onClick={copy}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          if (e.key === " ") e.preventDefault();
          copy();
        }
      }}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "12px 16px",
        borderBottom: "1px solid var(--border)",
        cursor: "pointer",
        transition: "background .15s ease",
      }}
    >
      <ExtChip ext="TXT" size={32} fontSize={9} radius={8} isTxt />
      <div style={{ flex: 1, minWidth: 0 }}>
        <span style={{ fontSize: 12.5, fontWeight: 600, color: "var(--ink2)" }}>
          {tr.name}
        </span>
        <DirBadge
          dir={tr.direction}
          style={{ marginLeft: 8, verticalAlign: 1 }}
        />
        <span style={{ fontSize: 11, color: "var(--muted)", marginLeft: 6 }}>
          {tr.peerName ?? ""}
        </span>
      </div>
      {/* biome-ignore lint/a11y/useSemanticElements: styled copy action kept — a <button> would change the chip's styling; made keyboard-operable via role/tabIndex/onKeyDown */}
      <span
        role="button"
        tabIndex={0}
        onClick={(e) => {
          e.stopPropagation();
          copy();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            if (e.key === " ") e.preventDefault();
            e.stopPropagation();
            copy();
          }
        }}
        style={{
          fontSize: 11,
          color: "var(--accent-ink)",
          cursor: "pointer",
          flex: "none",
        }}
      >
        {t("common.copy")}
      </span>
      <span
        style={{
          fontSize: 11,
          color: "var(--muted)",
          flex: "none",
          width: 70,
          textAlign: "right",
        }}
      >
        {tr.doneAt ? fmtWhen(tr.doneAt) : ""}
      </span>
    </div>
  );
});

export default function TransfersPage() {
  const { t } = useTranslation();
  const transfers = useTransfers((s) => s.transfers);
  const [filter, setFilter] = useState<Filter>("all");

  const list = transferList(transfers);
  // "queued" is an in-progress state (parked on the concurrency gate), so it
  // belongs in Running alongside "active" — only terminal rows go to History.
  // Text records are always terminal (history-only), so keep them out of Running.
  const running = list.filter(
    (x) =>
      (x.status === "active" || x.status === "queued") && x.kind !== "text",
  );
  const history = list.filter(
    (x) => x.status === "done" || x.status === "error",
  );

  return (
    <div
      className="scroll-y"
      style={{ flex: 1, animation: "lbFade .18s ease" }}
    >
      <div
        style={{
          maxWidth: 880,
          margin: "0 auto",
          padding: "22px 24px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
        }}
      >
        <div style={{ display: "flex", justifyContent: "flex-end" }}>
          <Segmented<Filter>
            options={[
              { key: "all", label: t("transfers.filterAll") },
              { key: "active", label: t("transfers.filterActive") },
              { key: "done", label: t("transfers.filterDone") },
            ]}
            value={filter}
            onChange={setFilter}
          />
        </div>
        {filter !== "done" && (
          <div className="card">
            <div style={cardHead}>
              <span
                style={{
                  fontSize: 12.5,
                  fontWeight: 650,
                  color: "var(--ink2)",
                }}
              >
                {t("transfers.running")}
              </span>
              <span style={{ fontSize: 11, color: "var(--muted)" }}>
                {t("transfers.runningCount", { n: running.length })}
              </span>
            </div>
            {running.length === 0 ? (
              <div style={emptyNote}>{t("transfers.noRunning")}</div>
            ) : (
              running.map((x) => <RunningRow key={x.sessionId} tr={x} />)
            )}
          </div>
        )}
        {filter !== "active" && (
          <div className="card">
            <div style={cardHead}>
              <span
                style={{
                  fontSize: 12.5,
                  fontWeight: 650,
                  color: "var(--ink2)",
                }}
              >
                {t("transfers.history")}
              </span>
              <span style={{ fontSize: 11, color: "var(--muted)" }}>
                {t("transfers.historyCount", { n: history.length })}
              </span>
            </div>
            {history.length === 0 ? (
              <div style={emptyNote}>{t("transfers.noHistory")}</div>
            ) : (
              history.map((x) =>
                x.kind === "text" ? (
                  <TextRow key={x.sessionId} tr={x} />
                ) : (
                  <HistoryRow key={x.sessionId} tr={x} />
                ),
              )
            )}
          </div>
        )}
      </div>
    </div>
  );
}
