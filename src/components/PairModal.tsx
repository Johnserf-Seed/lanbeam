import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import i18n from "../i18n";
import * as api from "../bridge/api";
import type { PairJoinedEvent } from "../bridge/api";
import { showToast, useData, useOverlays } from "../lib/store";
import { copyText, errText } from "../lib/sendops";
import Qr from "./Qr";
import { ModalHead } from "./ui";

/** 配对新设备 — host shows a real 6-digit code (start_pairing) + a scannable
 *  payload and waits for pair_joined to swap to the success state. A compact
 *  joiner section (a deliberate addition beyond the host-only mock) lets this
 *  device redeem another's code via join_by_code. */
export default function PairModal() {
  const { t } = useTranslation();
  const pairOpen = useOverlays((s) => s.pairOpen);
  const setPair = useOverlays((s) => s.setPair);

  // Host side: the invitation this device is showing.
  const [code, setCode] = useState("");
  const [qr, setQr] = useState("");
  // Success state, set by the pair_joined event when a device redeems our code.
  const [joined, setJoined] = useState<{ name: string; sas: string } | null>(
    null,
  );
  // Joiner side: redeem another device's code.
  const [joinAddr, setJoinAddr] = useState("");
  const [joinCode, setJoinCode] = useState("");
  const [joining, setJoining] = useState(false);
  // True only when the pointer went down on the scrim itself; a drag that
  // starts inside the modal and ends on the scrim dispatches click on the
  // scrim, which must not dismiss the modal.
  const scrimDown = useRef(false);

  // On open: mint a fresh invitation and listen for a device joining. On close
  // (effect cleanup or the close handler) the code is cancelled below.
  useEffect(() => {
    if (!pairOpen) return;
    // Clear the previous (now-cancelled) invite so a reopen never shows a stale
    // code/QR a scanner could redeem before startPairing() returns a fresh one.
    setCode("");
    setQr("");
    setJoined(null);
    // A deep link may have staged a lanbeam:// invitation. Consume it once to
    // pre-fill the join field (so a later manual reopen starts clean) and cue
    // the user to review + confirm — never auto-join an untrusted link.
    const prefill = useOverlays.getState().pairPrefill;
    setJoinAddr(prefill ?? "");
    if (prefill) {
      useOverlays.getState().setPairPrefill(null);
      showToast(i18n.t("pair.linkLoaded"));
    }
    setJoinCode("");
    setJoining(false);
    let live = true;
    void api
      .startPairing()
      .then((inv) => {
        if (live) {
          setCode(inv.code);
          setQr(inv.qr);
        }
      })
      .catch(() => {});
    const off = api.onEvent<PairJoinedEvent>("pair_joined", (p) => {
      setJoined({ name: p.name, sas: p.sas });
    });
    return () => {
      live = false;
      off();
    };
  }, [pairOpen]);

  if (!pairOpen) return null;

  // Closing invalidates the active code so a stale invite can't be redeemed.
  const close = () => {
    void api.cancelPairing();
    setPair(false);
  };

  const regen = () => {
    void api.startPairing().then((inv) => {
      setCode(inv.code);
      setQr(inv.qr);
    });
  };

  const doJoin = async () => {
    const addr = joinAddr.trim();
    if (!addr || joining) return;
    // A join target is an ip[:port] or a lanbeam:// link — both contain a "." or
    // "://". A bare 6-digit code pasted into the address field (a common mix-up)
    // has neither, so catch it with a clear localized hint rather than a raw
    // backend "not a valid address" error. The backend still validates for real.
    if (!addr.includes(".") && !addr.includes("://")) {
      showToast(t("pair.badAddr"));
      return;
    }
    setJoining(true);
    try {
      const r = await api.joinByCode(addr, joinCode.trim());
      // join_by_code records the just-paired peer in the manual table; discovery
      // may never announce it (a different subnet / discovery off — the exact
      // case code/IP pairing exists for), so it surfaces through the merged
      // list_discovered_devices, not a discovery event. Re-query it now so the
      // peer shows in the radar / SendModal / QuickTextModal immediately instead
      // of waiting for the next discovery tick. Mirrors DevicesPage.tryDirect.
      const list = await api.listDiscoveredDevices();
      useData.getState().setDevices(list);
      showToast(t("pair.joinedToast", { name: r.name }));
      close();
    } catch (e) {
      showToast(t("pair.joinFailed", { err: errText(e) }));
      setJoining(false);
    }
  };

  const fmtCode = (c: string) =>
    c.length === 6 ? `${c.slice(0, 3)} ${c.slice(3)}` : c;

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc
    // biome-ignore lint/a11y/useKeyWithClickEvents: same
    <div
      className="scrim"
      style={{ zIndex: 52 }}
      onMouseDown={(e) => {
        scrimDown.current = e.target === e.currentTarget;
      }}
      onClick={() => {
        if (scrimDown.current) close();
      }}
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: not an interactive control — stops backdrop clicks inside the modal from dismissing it (click-away guard) */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
      <div
        className="modal"
        style={{ width: 436, fontFamily: "var(--font)" }}
        onClick={(e) => e.stopPropagation()}
      >
        <ModalHead
          title={t("pair.title")}
          sub={t("pair.sub")}
          onClose={close}
        />

        {joined ? (
          /* ── success: a device redeemed our code ─────────────────────── */
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 9,
              padding: "24px 20px 20px",
              animation: "lbFade .2s ease",
            }}
          >
            <div
              style={{
                width: 44,
                height: 44,
                borderRadius: "50%",
                background: "var(--success-soft)",
                color: "var(--success)",
                display: "grid",
                placeItems: "center",
                fontSize: 19,
                fontWeight: 700,
              }}
            >
              ✓
            </div>
            <div
              style={{ fontSize: 13.5, fontWeight: 650, color: "var(--ink2)" }}
            >
              {t("pair.joined", { name: joined.name })}
            </div>
            <div
              style={{
                fontSize: 11.5,
                color: "var(--muted)",
                textAlign: "center",
              }}
            >
              {t("pair.joinedSub")}
            </div>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 11,
                color: "var(--muted2)",
              }}
            >
              {t("pair.sas", { sas: joined.sas })}
            </div>
            <button
              type="button"
              className="btn primary"
              style={{ padding: "0 18px", marginTop: 4 }}
              onClick={close}
            >
              {t("pair.done")}
            </button>
          </div>
        ) : (
          <>
            {/* ── host: our code + scannable payload ────────────────────── */}
            <div
              style={{
                display: "flex",
                gap: 16,
                padding: "16px 20px 6px",
                alignItems: "center",
                animation: "lbFade .18s ease",
              }}
            >
              <div
                style={{
                  flex: "none",
                  display: "flex",
                  flexDirection: "column",
                  alignItems: "center",
                  gap: 7,
                }}
              >
                {/* Real QR of the live lanbeam:// pairing deep-link (device +
                 *  addr + code) the joiner scans to auto-fill. Rendered only
                 *  once start_pairing has returned the payload; clicking still
                 *  copies the link so it can be shared out of band too. */}
                {/* biome-ignore lint/a11y/useSemanticElements: styled QR wrapper, not a native button — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                <div
                  title={qr}
                  role="button"
                  tabIndex={0}
                  style={{ cursor: qr ? "pointer" : "default" }}
                  onClick={() => {
                    if (!qr) return;
                    copyText(qr);
                    showToast(t("pair.linkCopied"));
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      if (!qr) return;
                      copyText(qr);
                      showToast(t("pair.linkCopied"));
                    }
                  }}
                >
                  <Qr value={qr} size={118} radius={10} />
                </div>
                <div style={{ fontSize: 10.5, color: "var(--muted)" }}>
                  {t("pair.scanHint")}
                </div>
              </div>
              <div
                style={{
                  flex: 1,
                  display: "flex",
                  flexDirection: "column",
                  gap: 6,
                }}
              >
                <div
                  style={{
                    fontSize: 10.5,
                    fontWeight: 600,
                    letterSpacing: ".07em",
                    color: "var(--muted)",
                  }}
                >
                  {t("pair.orCode")}
                </div>
                <div
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 27,
                    fontWeight: 600,
                    letterSpacing: ".08em",
                    color: "var(--accent-ink)",
                    minHeight: 34,
                  }}
                >
                  {fmtCode(code)}
                </div>
                <div
                  style={{
                    fontSize: 10.5,
                    color: "var(--muted)",
                    lineHeight: 1.6,
                  }}
                >
                  {t("pair.validity")}
                </div>
                {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native button — keeps the custom layout/styling while staying keyboard-operable via role/tabIndex/onKeyDown */}
                <span
                  role="button"
                  tabIndex={0}
                  style={{
                    fontSize: 11,
                    color: "var(--accent-ink)",
                    cursor: "pointer",
                  }}
                  onClick={regen}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      regen();
                    }
                  }}
                >
                  {t("pair.regen")}
                </span>
              </div>
            </div>
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "0 20px 14px",
                fontSize: 11,
                color: "var(--muted2)",
              }}
            >
              <span
                style={{
                  width: 8,
                  height: 8,
                  borderRadius: "50%",
                  background: "var(--accent)",
                  animation: "lbBlink 1.4s ease-in-out infinite",
                  flex: "none",
                }}
              />
              {t("pair.waiting")}
            </div>

            {/* ── joiner: redeem another device's code (beyond the mock) ─── */}
            <div
              style={{
                borderTop: "1px solid var(--border)",
                padding: "12px 20px 14px",
                display: "flex",
                flexDirection: "column",
                gap: 8,
              }}
            >
              <div
                style={{
                  fontSize: 10.5,
                  fontWeight: 600,
                  letterSpacing: ".07em",
                  color: "var(--muted)",
                }}
              >
                {t("pair.joinTitle")}
              </div>
              <div style={{ display: "flex", gap: 8 }}>
                <input
                  className="input"
                  style={{ flex: 1, minWidth: 0 }}
                  value={joinAddr}
                  placeholder={t("pair.joinAddr")}
                  onChange={(e) => setJoinAddr(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void doJoin();
                  }}
                />
                <input
                  className="input"
                  style={{
                    width: 96,
                    flex: "none",
                    fontFamily: "var(--mono)",
                    letterSpacing: ".08em",
                  }}
                  value={joinCode}
                  placeholder={t("pair.joinCode")}
                  inputMode="numeric"
                  onChange={(e) => setJoinCode(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void doJoin();
                  }}
                />
                <button
                  type="button"
                  className={`btn primary${joinAddr.trim() && !joining ? "" : " off"}`}
                  style={{ padding: "0 14px", flex: "none" }}
                  onClick={() => void doJoin()}
                >
                  {t("pair.joinBtn")}
                </button>
              </div>
            </div>
          </>
        )}
        <div className="modal-foot">{t("pair.foot")}</div>
      </div>
    </div>
  );
}
