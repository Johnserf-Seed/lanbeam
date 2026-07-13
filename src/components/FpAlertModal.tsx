import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import { fmtSas } from "../lib/format";
import { errText } from "../lib/sendops";
import {
  showToast,
  trustList,
  useData,
  useOverlays,
  useTrust,
} from "../lib/store";

/** Full display fingerprint: 4 groups of 4 alnum chars from the device key. */
function fpFull(deviceId: string): string {
  const hex = deviceId
    .replace(/[^a-zA-Z0-9]/g, "")
    .toUpperCase()
    .padEnd(16, "0");
  return [0, 4, 8, 12].map((i) => hex.slice(i, i + 4)).join(" ");
}

function CloseBtn({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        width: 28,
        height: 28,
        borderRadius: 8,
        border: "none",
        background: "none",
        color: "var(--muted)",
        fontSize: 15,
        cursor: "pointer",
        flex: "none",
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = "var(--hover)")}
      onMouseLeave={(e) => (e.currentTarget.style.background = "none")}
    >
      ×
    </button>
  );
}

/** 指纹变化警告 — real feature: a remembered device's name reappeared under a
 *  different key. Offers removing trust or re-verifying the SAS against the
 *  new key and migrating the trust record. */
export default function FpAlertModal() {
  const { t } = useTranslation();
  const fpAlert = useOverlays((s) => s.fpAlert);
  const setFpAlert = useOverlays((s) => s.setFpAlert);
  const records = useTrust((s) => s.records);
  const removeTrust = useTrust((s) => s.remove);
  const migrate = useTrust((s) => s.migrate);
  const devices = useData((s) => s.devices);
  const [reverifying, setReverifying] = useState(false);

  const record = fpAlert ? records[fpAlert.deviceId] : undefined;
  const imposterId = fpAlert
    ? trustList(devices, records).find((d) => d.deviceId === fpAlert.deviceId)
        ?.fpChanged?.newDeviceId
    : undefined;

  // The alert is stale once the record is gone or the new key disappeared.
  const stale = !!fpAlert && (!record || !imposterId);
  useEffect(() => {
    if (stale) setFpAlert(null);
  }, [stale, setFpAlert]);

  if (!fpAlert || !record || !imposterId) return null;
  const close = () => setFpAlert(null);
  const name = record.name;

  const remove = (toastKey: string) => {
    removeTrust(fpAlert.deviceId);
    showToast(t(toastKey));
    close();
  };

  const reverify = () => {
    if (reverifying) return;
    setReverifying(true);
    // The handshake has unbounded latency; only act on the result if this warn
    // alert is still the one on screen — the user may have dismissed it, or a
    // different device's alert may have replaced it meanwhile.
    const stillCurrent = () => {
      const cur = useOverlays.getState().fpAlert;
      return !!cur && cur.deviceId === fpAlert.deviceId && cur.step === "warn";
    };
    api
      .connectDevice(imposterId)
      .then((sas) => {
        if (stillCurrent())
          setFpAlert({ deviceId: fpAlert.deviceId, step: "verify", sas });
      })
      .catch((e) => {
        if (stillCurrent()) showToast(errText(e));
      })
      .finally(() => setReverifying(false));
  };

  const match = () => {
    migrate(fpAlert.deviceId, imposterId, name);
    showToast(t("fp.matchToast"));
    close();
  };

  return (
    <div className="scrim" style={{ zIndex: 62 }}>
      <div className="modal" style={{ width: 404, fontFamily: "var(--font)" }}>
        {fpAlert.step === "warn" ? (
          <div style={{ animation: "lbFade .18s ease" }}>
            <div
              style={{
                display: "flex",
                gap: 11,
                padding: "18px 20px 0",
                alignItems: "flex-start",
              }}
            >
              <div
                style={{
                  width: 34,
                  height: 34,
                  borderRadius: 10,
                  background: "var(--danger-soft)",
                  color: "var(--danger)",
                  display: "grid",
                  placeItems: "center",
                  fontSize: 16,
                  fontWeight: 700,
                  flex: "none",
                }}
              >
                !
              </div>
              <div style={{ flex: 1 }}>
                <div
                  style={{
                    fontSize: 14,
                    fontWeight: 650,
                    color: "var(--ink2)",
                  }}
                >
                  {t("fp.warnTitle", { name })}
                </div>
                <div
                  style={{
                    fontSize: 11.5,
                    color: "var(--muted)",
                    marginTop: 2,
                  }}
                >
                  {t("fp.warnSub")}
                </div>
              </div>
              <CloseBtn onClick={close} />
            </div>
            <div
              style={{
                margin: "14px 20px 0",
                background: "var(--sidebar)",
                borderRadius: 10,
                padding: "11px 13px",
                display: "flex",
                flexDirection: "column",
                gap: 6,
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "baseline",
                  gap: 10,
                }}
              >
                <span
                  style={{
                    fontSize: 10.5,
                    color: "var(--muted)",
                    flex: "none",
                  }}
                >
                  {t("fp.before")}
                </span>
                <span
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 11,
                    color: "var(--muted)",
                    textDecoration: "line-through",
                  }}
                >
                  {fpFull(record.deviceId)}
                </span>
              </div>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "baseline",
                  gap: 10,
                }}
              >
                <span
                  style={{
                    fontSize: 10.5,
                    color: "var(--danger)",
                    fontWeight: 600,
                    flex: "none",
                  }}
                >
                  {t("fp.now")}
                </span>
                <span
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 11,
                    fontWeight: 600,
                    color: "var(--danger)",
                  }}
                >
                  {fpFull(imposterId)}
                </span>
              </div>
            </div>
            <div
              style={{
                padding: "12px 20px 0",
                fontSize: 11.5,
                lineHeight: 1.65,
                color: "var(--muted2)",
              }}
            >
              {t("fp.explain")}
            </div>
            <div style={{ display: "flex", gap: 9, padding: "16px 20px 18px" }}>
              <button
                type="button"
                className="btn danger"
                style={{ flex: 1, height: 34, borderRadius: 9, fontSize: 12 }}
                onClick={() => remove("fp.removedToast")}
              >
                {t("fp.removeTrust")}
              </button>
              <button
                type="button"
                className="btn primary"
                style={{ flex: 1.4, height: 34, borderRadius: 9, fontSize: 12 }}
                disabled={reverifying}
                onClick={reverify}
              >
                {t("fp.reverify")}
              </button>
            </div>
          </div>
        ) : (
          <div style={{ animation: "lbFade .18s ease" }}>
            <div
              style={{
                display: "flex",
                alignItems: "flex-start",
                gap: 10,
                padding: "18px 20px 0",
              }}
            >
              <div style={{ flex: 1 }}>
                <div
                  style={{
                    fontSize: 14,
                    fontWeight: 650,
                    color: "var(--ink2)",
                  }}
                >
                  {t("fp.verifyTitle", { name })}
                </div>
                <div
                  style={{
                    fontSize: 11.5,
                    color: "var(--muted)",
                    marginTop: 2,
                  }}
                >
                  {t("fp.verifySub")}
                </div>
              </div>
              <CloseBtn onClick={close} />
            </div>
            <div
              style={{
                margin: "14px 20px 0",
                background: "var(--accent-soft)",
                borderRadius: 12,
                padding: "15px 22px",
                textAlign: "center",
              }}
            >
              <div
                style={{
                  fontSize: 10.5,
                  color: "var(--muted2)",
                  letterSpacing: ".04em",
                }}
              >
                {t("fp.sasLabel")}
              </div>
              <div
                style={{
                  fontFamily: "var(--mono)",
                  fontSize: 24,
                  fontWeight: 600,
                  color: "var(--accent-ink)",
                  marginTop: 8,
                  letterSpacing: ".06em",
                }}
              >
                {fmtSas(fpAlert.sas)}
              </div>
            </div>
            <div
              style={{
                padding: "10px 20px 0",
                fontFamily: "var(--mono)",
                fontSize: 10.5,
                color: "var(--muted)",
                textAlign: "center",
              }}
            >
              {t("fp.verifyNote")}
            </div>
            <div style={{ display: "flex", gap: 9, padding: "16px 20px 18px" }}>
              <button
                type="button"
                className="btn danger"
                style={{ flex: 1, height: 34, borderRadius: 9, fontSize: 12 }}
                onClick={() => remove("fp.mismatchToast")}
              >
                {t("fp.mismatch")}
              </button>
              <button
                type="button"
                className="btn primary"
                style={{ flex: 1.4, height: 34, borderRadius: 9, fontSize: 12 }}
                onClick={match}
              >
                {t("fp.match")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
