/** 接收确认 — stacked incoming-request cards, top-right of the window.
 *  Styles transcribed from「LanBeam 原型 v2」lines 1150-1186. */
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import type { IncomingRequest } from "../bridge/api";
import {
  notify,
  shortFp,
  showToast,
  trustName,
  useData,
  useOverlays,
  useTransfers,
  useTrust,
} from "../lib/store";
import { errText } from "../lib/sendops";
import { fmtBytes, fmtSas } from "../lib/format";

/** Where the stack sits: above the page and the detail drawer (45/46), BELOW
 *  every modal's scrim (50–56, and the decision dialogs at 62). At 60 it drew
 *  over an open modal's own controls; under the scrim it waits, dimmed, and is
 *  back in front the moment the modal closes. */
const STACK_Z = 49;

/** "3 waiting" beside the sender's name. */
const queueBadge = {
  fontSize: 10,
  fontWeight: 600,
  color: "var(--accent-ink)",
  background: "var(--accent-soft)",
  borderRadius: 99,
  padding: "2px 8px",
  flex: "none",
} as const;

const chipStyle = {
  fontFamily: "var(--mono)",
  fontSize: 10.5,
  color: "var(--muted2)",
  background: "var(--sidebar)",
  borderRadius: 6,
  padding: "4px 8px",
  maxWidth: "100%",
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
} as const;

export default function IncomingStack() {
  const { t } = useTranslation();
  const incomings = useTransfers((s) => s.incomings);
  const devices = useData((s) => s.devices);
  const records = useTrust((s) => s.records);
  const [trustPending, setTrustPending] = useState(false);
  const [leaving, setLeaving] = useState<"accept" | "decline" | null>(null);
  const [stackHover, setStackHover] = useState(false);
  const [cardHover, setCardHover] = useState(false);
  // 收起: the card folds to one line so it stops covering the page under it.
  const [collapsed, setCollapsed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(timer.current), []);

  // The DOM is removed while the pointer is still inside when the last card is
  // accepted/declined — no mouseleave ever fires, so drop the hover flags here
  // or the next incoming would mount already in the hover state.
  useEffect(() => {
    if (incomings.length === 0) {
      setStackHover(false);
      setCardHover(false);
    }
  }, [incomings.length]);

  // trustPending is stack-level state, but the "trust this device" intent it
  // holds is only ever meant for the card the user ticked it on. If the front
  // card is swapped out from under us (e.g. its session errors and AppShell
  // removes it, promoting the queued request), reset the checkbox so an Accept
  // aimed at the old peer can't silently grant trust+autoAccept to the new one.
  // frontSessionId is only a re-run TRIGGER (reset the checkbox when the front
  // card swaps out), deliberately not read in the effect body.
  // A new front card also arrives unfolded: a new request is news, even to
  // someone who folded away the last one.
  const frontSessionId = incomings[0]?.sessionId;
  useEffect(() => {
    setTrustPending(false);
    setCollapsed(false);
  }, [frontSessionId]);

  const front = incomings[0];
  if (!front) return null;

  // Display name: the user's rename wins, then the name the sender declared
  // for this session (M4.2), then discovery, then the fingerprint.
  const nameOf = (r: IncomingRequest): string =>
    trustName(records[r.deviceId]) ||
    r.senderName ||
    devices.find((d) => d.deviceId === r.deviceId)?.name ||
    shortFp(r.deviceId);

  const peerName = nameOf(front);

  const accept = () => {
    if (leaving) return;
    const r = front;
    const wantTrust = trustPending;
    setLeaving("accept");
    timer.current = setTimeout(() => {
      const tf = useTransfers.getState();
      // Name collision under the "ask" policy (M6.5): the SAS was verified on
      // this card, but the keep-both/overwrite decision belongs to the
      // ConflictModal, which issues the single accept+choice reply. Trust and
      // meta are deferred to it too, so a "cancel receiving" there grants nothing.
      if (r.conflicts?.length && r.conflictPolicy === "ask") {
        useOverlays.getState().setConflict({ request: r, peerName, wantTrust });
        tf.removeIncoming(r.sessionId);
        setTrustPending(false);
        setLeaving(null);
        return;
      }
      if (wantTrust) {
        const trust = useTrust.getState();
        trust.setTrust({ deviceId: r.deviceId, name: peerName }, true);
        const rec = useTrust.getState().records[r.deviceId];
        if (rec && !rec.autoAccept) trust.toggleAuto(r.deviceId);
      }
      tf.acceptMeta(r, peerName);
      api
        .replyFileRequest(r.sessionId, true)
        .catch((e) => showToast(errText(e)));
      tf.removeIncoming(r.sessionId);
      setTrustPending(false);
      setLeaving(null);
      showToast(
        t(wantTrust ? "incoming.acceptTrustToast" : "incoming.acceptToast", {
          name: peerName,
        }),
      );
      notify();
    }, 240);
  };

  const decline = () => {
    if (leaving) return;
    const r = front;
    setLeaving("decline");
    timer.current = setTimeout(() => {
      api
        .replyFileRequest(r.sessionId, false)
        .catch((e) => showToast(errText(e)));
      useTransfers.getState().removeIncoming(r.sessionId);
      setTrustPending(false);
      setLeaving(null);
      showToast(t("incoming.declineToast", { name: peerName }));
    }, 240);
  };

  // Folded: one line in the same corner — who is waiting, and how many. The
  // request is still pending; one click brings the card back to answer it.
  if (collapsed) {
    return (
      <div style={{ position: "fixed", right: 22, top: 76, zIndex: STACK_Z }}>
        <button
          type="button"
          aria-label={t("incoming.expandLabel", { name: peerName })}
          onClick={() => setCollapsed(false)}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            maxWidth: 302,
            padding: "6px 13px 6px 7px",
            background: "var(--panel)",
            border: "1px solid var(--border2)",
            borderRadius: 99,
            boxShadow: "var(--shadow)",
            color: "var(--ink)",
            font: "inherit",
            cursor: "pointer",
            animation: "lbFade .2s ease",
          }}
        >
          <span
            style={{
              width: 22,
              height: 22,
              borderRadius: "50%",
              background: "var(--accent-soft)",
              color: "var(--accent-ink)",
              display: "grid",
              placeItems: "center",
              fontSize: 11,
              fontWeight: 600,
              flex: "none",
            }}
          >
            {peerName.trim().charAt(0)}
          </span>
          <span
            style={{
              fontSize: 12.5,
              fontWeight: 600,
              color: "var(--ink2)",
              minWidth: 0,
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {peerName}
          </span>
          {incomings.length > 1 && (
            <span style={queueBadge}>
              {t("incoming.queue", { n: incomings.length })}
            </span>
          )}
          <span
            style={{
              fontSize: 11,
              fontWeight: 600,
              color: "var(--accent-ink)",
              flex: "none",
            }}
          >
            {t("incoming.expand")}
          </span>
        </button>
      </div>
    );
  }

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: hover-only presentational stack container — pointer hover expands the card fan, no keyboard action to expose.
    <div
      onMouseEnter={() => setStackHover(true)}
      onMouseLeave={() => setStackHover(false)}
      style={{
        position: "fixed",
        right: 22,
        top: 76,
        zIndex: STACK_Z,
        width: 302,
      }}
    >
      {incomings.slice(1, 3).map((q, i) => {
        const top = (i + 1) * (stackHover ? 26 : 10);
        const inset = (i + 1) * (stackHover ? 3 : 7);
        return (
          <div
            key={q.sessionId}
            style={{
              position: "absolute",
              top,
              bottom: -top,
              left: inset,
              right: inset,
              zIndex: 2 - i,
              background: "var(--panel)",
              border: "1px solid var(--border)",
              borderRadius: 14,
              boxShadow: "var(--shadow)",
              transition:
                "top .25s ease,bottom .25s ease,left .25s ease,right .25s ease",
              animation: "lbFade .25s ease",
            }}
          />
        );
      })}

      {/* biome-ignore lint/a11y/noStaticElementInteractions: hover-only presentational card — pointer hover shifts the border color, no keyboard action to expose. */}
      <div
        key={front.sessionId}
        onMouseEnter={() => setCardHover(true)}
        onMouseLeave={() => setCardHover(false)}
        style={{
          position: "relative",
          zIndex: 3,
          background: "var(--panel)",
          border: `1px solid ${cardHover ? "var(--border2)" : "var(--border)"}`,
          borderRadius: 14,
          padding: "14px 16px",
          boxShadow: "var(--shadow)",
          animation: leaving
            ? "lbCardOut .24s ease forwards"
            : "lbCard .3s ease",
          color: "var(--ink)",
          fontFamily: "var(--font)",
          transition: "border-color .18s ease",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div
            style={{
              width: 32,
              height: 32,
              borderRadius: "50%",
              background: "var(--accent-soft)",
              color: "var(--accent-ink)",
              display: "grid",
              placeItems: "center",
              fontSize: 13,
              fontWeight: 600,
              flex: "none",
            }}
          >
            {peerName.trim().charAt(0)}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div
              style={{
                fontSize: 13,
                fontWeight: 600,
                color: "var(--ink2)",
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {peerName}
            </div>
            <div style={{ fontSize: 11, color: "var(--muted)" }}>
              {t("incoming.subLine", {
                count: front.fileCount,
                size: fmtBytes(front.totalSize),
              })}
            </div>
          </div>
          {incomings.length > 1 && (
            <span style={queueBadge}>
              {t("incoming.queue", { n: incomings.length })}
            </span>
          )}
          <button
            type="button"
            title={t("incoming.collapse")}
            aria-label={t("incoming.collapse")}
            disabled={leaving !== null}
            onClick={() => setCollapsed(true)}
            style={{
              width: 22,
              height: 22,
              padding: 0,
              border: "none",
              borderRadius: 6,
              background: "none",
              color: "var(--muted2)",
              fontSize: 15,
              lineHeight: 1,
              cursor: "pointer",
              flex: "none",
            }}
          >
            –
          </button>
        </div>

        <div
          style={{ display: "flex", gap: 6, marginTop: 11, flexWrap: "wrap" }}
        >
          <span style={chipStyle}>{front.files[0]?.name ?? ""}</span>
          {front.fileCount > 1 && (
            <span style={chipStyle}>
              {t("incoming.moreLabel", { n: front.fileCount - 1 })}
            </span>
          )}
        </div>

        <div
          style={{
            marginTop: 11,
            background: "var(--accent-soft)",
            borderRadius: 10,
            padding: "9px 12px",
            textAlign: "center",
          }}
        >
          <div
            style={{
              fontSize: 10,
              color: "var(--muted2)",
              letterSpacing: ".04em",
            }}
          >
            {t("incoming.sasLabel")}
          </div>
          <div
            style={{
              fontFamily: "var(--mono)",
              fontSize: 16,
              fontWeight: 600,
              color: "var(--accent-ink)",
              marginTop: 4,
              letterSpacing: ".05em",
            }}
          >
            {fmtSas(front.sas)}
          </div>
        </div>

        <label
          style={{
            display: "flex",
            alignItems: "center",
            gap: 7,
            marginTop: 11,
            fontSize: 11.5,
            color: "var(--muted2)",
            cursor: "pointer",
          }}
        >
          <input
            type="checkbox"
            checked={trustPending}
            onChange={(e) => setTrustPending(e.target.checked)}
            style={{ accentColor: "var(--accent)", margin: 0 }}
          />
          {t("incoming.trustCheck")}
        </label>

        <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
          <button
            type="button"
            className="btn"
            style={{ flex: 1, height: 30 }}
            onClick={decline}
          >
            {t("incoming.decline")}
          </button>
          <button
            type="button"
            className="btn primary"
            style={{ flex: 1, height: 30, fontSize: 12 }}
            onClick={accept}
          >
            {t("incoming.accept")}
          </button>
        </div>
      </div>
    </div>
  );
}
