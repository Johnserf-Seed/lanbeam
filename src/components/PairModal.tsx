import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import i18n from "../i18n";
import * as api from "../bridge/api";
import type { PairJoinedEvent } from "../bridge/api";
import { showToast, useData, useOverlays, useTrust } from "../lib/store";
import { copyText, errText } from "../lib/sendops";
import { fmtSas } from "../lib/format";
import Qr from "./Qr";
import { ModalHead } from "./ui";

/** 配对新设备 — host shows a real 6-digit code (start_pairing) + a scannable
 *  payload; a compact joiner section redeems another device's code.
 *
 *  BOTH roles end at the same place: the SAS compare step. Redeeming a code and
 *  trusting a device are two different decisions, and only the second one needs
 *  a human — the SAS is the sole defence against a machine-in-the-middle, and a
 *  code nobody read is not a check. So neither `join_by_code` nor the host's
 *  pairing handler grants any trust; it is recorded HERE, through the same
 *  `useTrust.setTrust` path the trust circle uses (which is what also turns on
 *  auto-accept), and only once the user says the two screens agree. Dismissing
 *  the compare step trusts nothing. */
export default function PairModal() {
  const { t } = useTranslation();
  const pairOpen = useOverlays((s) => s.pairOpen);
  const setPair = useOverlays((s) => s.setPair);

  // Host side: the invitation this device is showing.
  const [code, setCode] = useState("");
  const [qr, setQr] = useState("");
  // The compare step. Reached from BOTH sides — the host via `pair_joined`, the
  // joiner via join_by_code's return — and carrying the same handshake-derived
  // SAS, which is the point: two screens, one number.
  const [compare, setCompare] = useState<{
    deviceId: string;
    name: string;
    sas: string;
  } | null>(null);
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
    setCompare(null);
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
      // Don't leave a blank code and a blinking "waiting for a device…" behind a
      // swallowed error — there is nothing to wait for.
      .catch((e) => {
        if (live) showToast(t("pair.codeFailed", { err: errText(e) }));
      });
    const off = api.onEvent<PairJoinedEvent>("pair_joined", (p) => {
      setCompare({ deviceId: p.deviceId, name: p.name, sas: p.sas });
    });
    return () => {
      live = false;
      off();
    };
  }, [pairOpen, t]);

  if (!pairOpen) return null;

  /** Tear down without a verdict. Invalidates the active code so a stale invite
   *  can't be redeemed later. */
  const dismiss = () => {
    void api.cancelPairing();
    setPair(false);
  };

  // Walking away from the compare step is a "no" — nothing was trusted. Say so,
  // or the user leaves believing they paired.
  const close = () => {
    if (compare) showToast(t("pair.unconfirmedToast"));
    dismiss();
  };

  /** The two screens agree → record the trust, through the SAME path the trust
   *  circle uses (so a paired device is trusted AND auto-accepting, exactly like
   *  one dragged into the circle — there is no second, divergent notion of
   *  trust for pairing to drift away from). */
  const acceptPeer = () => {
    if (!compare) return;
    useTrust
      .getState()
      .setTrust({ deviceId: compare.deviceId, name: compare.name }, true);
    showToast(t("pair.trustedToast", { name: compare.name }));
    dismiss();
  };

  const rejectPeer = () => {
    showToast(t("pair.mismatchToast"), undefined, 8000);
    dismiss();
  };

  const regen = () => {
    void api
      .startPairing()
      .then((inv) => {
        setCode(inv.code);
        setQr(inv.qr);
      })
      .catch((e) => showToast(t("pair.codeFailed", { err: errText(e) })));
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
      // join_by_code records the just-redeemed peer in the manual table; discovery
      // may never announce it (a different subnet / discovery off — the exact
      // case code/IP pairing exists for), so it surfaces through the merged
      // list_discovered_devices, not a discovery event. Re-query it now so the
      // peer shows in the radar / SendModal / QuickTextModal immediately instead
      // of waiting for the next discovery tick. Mirrors DevicesPage.tryDirect.
      // It shows up UNTRUSTED — being reachable and being trusted are separate,
      // and the second one is still one SAS compare away.
      const list = await api.listDiscoveredDevices();
      useData.getState().setDevices(list);
      setCompare({ deviceId: r.deviceId, name: r.name, sas: r.sas });
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

        {compare ? (
          /* ── compare: the code was redeemed, nothing is trusted yet ───── */
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 8,
              padding: "20px 20px 18px",
              animation: "lbFade .2s ease",
            }}
          >
            <div
              style={{ fontSize: 13.5, fontWeight: 650, color: "var(--ink2)" }}
            >
              {t("pair.compareTitle", { name: compare.name })}
            </div>
            <div
              style={{
                fontSize: 11.5,
                color: "var(--muted)",
                textAlign: "center",
              }}
            >
              {t("pair.compareBody")}
            </div>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 26,
                fontWeight: 600,
                letterSpacing: ".06em",
                color: "var(--accent-ink)",
                background: "var(--accent-soft)",
                borderRadius: 10,
                padding: "10px 18px",
                margin: "2px 0",
              }}
            >
              {fmtSas(compare.sas)}
            </div>
            <div
              style={{
                fontSize: 10.5,
                color: "var(--muted)",
                textAlign: "center",
                lineHeight: 1.6,
              }}
            >
              {t("pair.compareWarn")}
            </div>
            <div
              style={{ display: "flex", gap: 8, marginTop: 6, width: "100%" }}
            >
              <button
                type="button"
                className="btn danger"
                style={{ flex: "none", padding: "0 14px" }}
                onClick={rejectPeer}
              >
                {t("pair.mismatch")}
              </button>
              <button
                type="button"
                className="btn primary"
                style={{ flex: 1 }}
                onClick={acceptPeer}
              >
                {t("pair.match")}
              </button>
            </div>
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
