import { useEffect } from "react";
import { useTranslation } from "react-i18next";
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

/** 指纹变化警告 — a remembered device's name has reappeared under a different key.
 *
 *  It reports, and it offers two SAFE ways out. It does not offer to "re-verify
 *  and restore trust", which is what it used to do:
 *
 *  That flow dialled the new key, showed the resulting SAS, and told the user
 *  「两台设备屏幕应显示一致」. The other device showed NOTHING — `connect_device`
 *  hands the SAS back to the caller and the far end just swallows a Bye. So the
 *  number had nothing to be compared against, and pressing 「一致」 migrated trust
 *  onto the very key the alert existed to make you suspicious of. A code only one
 *  side can read is not a check; it is a formality that ends in a yes.
 *
 *  Trusting a key you have never seen is what PAIRING is for — and pairing now
 *  shows the same SAS on BOTH screens and needs both people to confirm it. So the
 *  way back is to pair, like with any new device. That is heavier, and it should
 *  be: the whole question here is whether the thing on the other end is yours. */
export default function FpAlertModal() {
  const { t } = useTranslation();
  const fpAlert = useOverlays((s) => s.fpAlert);
  const setFpAlert = useOverlays((s) => s.setFpAlert);
  const setPair = useOverlays((s) => s.setPair);
  const records = useTrust((s) => s.records);
  const removeTrust = useTrust((s) => s.remove);
  const devices = useData((s) => s.devices);

  const record = fpAlert ? records[fpAlert.deviceId] : undefined;
  const newDeviceId = fpAlert
    ? trustList(devices, records).find((d) => d.deviceId === fpAlert.deviceId)
        ?.fpChanged?.newDeviceId
    : undefined;
  const newDevice = devices.find((d) => d.deviceId === newDeviceId);

  // The alert is stale once the record is gone or the new key is no longer here.
  const stale = !!fpAlert && (!record || !newDevice);
  useEffect(() => {
    if (stale) setFpAlert(null);
  }, [stale, setFpAlert]);

  if (!fpAlert || !record || !newDevice) return null;
  const close = () => setFpAlert(null);
  const name = record.name;

  /* Delete the remembered device. The key it stood for is not coming back — and
     if the new one really is that machine, it will earn its own record by
     pairing. */
  const forgetOld = () => {
    removeTrust(fpAlert.deviceId);
    showToast(t("fp.removedToast"));
    close();
  };

  /* Pair with the new key, like the new device it is. PairModal shows the SAS on
     both screens and records trust only when a human says they match.
     `setPair(open, prefill)` in ONE call — the two-arg form exists because
     `setPair(true)` alone defaults the prefill to null and would wipe it. */
  const pair = () => {
    close();
    setPair(true, newDevice.address);
  };

  return (
    <div className="scrim" style={{ zIndex: 62 }}>
      <div
        className="modal"
        style={{
          width: 420,
          fontFamily: "var(--font)",
          animation: "lbFade .18s ease",
        }}
      >
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
              width: 30,
              height: 30,
              flex: "none",
              borderRadius: 9,
              background: "var(--danger-soft)",
              color: "var(--danger)",
              display: "grid",
              placeItems: "center",
              fontSize: 15,
              fontWeight: 700,
            }}
          >
            !
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div
              style={{ fontSize: 13.5, fontWeight: 650, color: "var(--ink2)" }}
            >
              {t("fp.warnTitle", { name })}
            </div>
            <div
              style={{ fontSize: 11.5, color: "var(--muted)", marginTop: 3 }}
            >
              {t("fp.warnSub")}
            </div>
          </div>
          <CloseBtn onClick={close} />
        </div>

        {/* The two keys, side by side. This is the whole evidence. */}
        <div
          style={{
            display: "flex",
            gap: 10,
            margin: "14px 20px 0",
            padding: "11px 13px",
            borderRadius: 10,
            background: "var(--sidebar)",
            border: "1px solid var(--border)",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 10, color: "var(--muted)" }}>
              {t("fp.before")}
            </div>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 11,
                color: "var(--muted2)",
                marginTop: 3,
                wordBreak: "break-all",
              }}
            >
              {fpFull(fpAlert.deviceId)}
            </div>
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 10, color: "var(--danger)" }}>
              {t("fp.now")}
            </div>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 11,
                color: "var(--danger)",
                marginTop: 3,
                wordBreak: "break-all",
              }}
            >
              {fpFull(newDevice.deviceId)}
            </div>
          </div>
        </div>

        <div
          style={{
            fontSize: 11.5,
            color: "var(--muted2)",
            lineHeight: 1.7,
            padding: "12px 20px 0",
          }}
        >
          {t("fp.explain")}
        </div>

        <div
          style={{
            display: "flex",
            gap: 8,
            justifyContent: "flex-end",
            padding: "14px 20px 16px",
          }}
        >
          <button
            type="button"
            className="btn danger"
            style={{ padding: "0 14px" }}
            onClick={forgetOld}
          >
            {t("fp.forgetOld")}
          </button>
          <button
            type="button"
            className="btn primary"
            style={{ padding: "0 16px" }}
            onClick={pair}
          >
            {t("fp.pairAgain")}
          </button>
        </div>
        <div className="modal-foot">{t("fp.foot")}</div>
      </div>
    </div>
  );
}
