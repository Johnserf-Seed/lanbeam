import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { showToast, useData, useOverlays } from "../lib/store";
import { errText, sendTextTracked } from "../lib/sendops";
import { ModalHead, Toggle } from "./ui";

/** ⌁ 快传文本 — sends a short text/link over the encrypted channel for real
 *  (send_text, M7.3). The clipboard toggle is a REQUEST to the receiver; their
 *  own clipboard-sharing consent decides whether it actually lands there. */
export default function QuickTextModal() {
  const { t } = useTranslation();
  const qtOpen = useOverlays((s) => s.qtOpen);
  const setQt = useOverlays((s) => s.setQt);
  const devices = useData((s) => s.devices);

  const [text, setText] = useState("");
  const [clip, setClip] = useState(true);
  const [target, setTarget] = useState("");
  const [sending, setSending] = useState(false);
  // True only when the pointer went down on the scrim itself; a drag that
  // starts inside the modal (e.g. selecting textarea text) and ends on the
  // scrim dispatches click on the scrim, which must not destroy the draft.
  const scrimDown = useRef(false);

  // Reset draft state every time the modal opens. A `lanbeam://text?t=…` deep
  // link may have staged a body — consume it ONCE (so a later manual open starts
  // clean) and drop it into the draft. It is only a pre-fill: the user still
  // picks the device and presses send, which is the whole reason a link is
  // allowed to touch this at all.
  useEffect(() => {
    if (qtOpen) {
      const prefill = useOverlays.getState().qtPrefill;
      setText(prefill ?? "");
      if (prefill) useOverlays.setState({ qtPrefill: null });
      setClip(true);
      setTarget("");
      setSending(false);
    }
  }, [qtOpen]);

  if (!qtOpen) return null;
  const close = () => setQt(false);
  const targetId = devices.some((d) => d.deviceId === target)
    ? target
    : (devices[0]?.deviceId ?? "");
  const targetName =
    devices.find((d) => d.deviceId === targetId)?.name ?? targetId;
  const canSend = !!text.trim() && !!targetId && !sending;

  const send = async () => {
    if (!canSend) return;
    setSending(true);
    try {
      await sendTextTracked(targetId, targetName, text.trim(), clip);
      showToast(t("qt.sentToast", { name: targetName }));
      close();
    } catch (e) {
      showToast(t("qt.sendFailed", { err: errText(e) }));
      setSending(false);
    }
  };

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
      {/* biome-ignore lint/a11y/noStaticElementInteractions: onClick only stops propagation so clicks inside the modal don't reach the backdrop — not an interactive control */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
      <div
        className="modal"
        style={{ width: 462, fontFamily: "var(--font)" }}
        onClick={(e) => e.stopPropagation()}
      >
        <ModalHead title={t("qt.title")} sub={t("qt.sub")} onClose={close} />
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "14px 20px 0",
          }}
        >
          <span style={{ fontSize: 12, color: "var(--muted2)", flex: "none" }}>
            {t("qt.sendTo")}
          </span>
          <select
            className="input"
            style={{ flex: 1 }}
            value={targetId}
            disabled={!devices.length}
            onChange={(e) => setTarget(e.target.value)}
          >
            {devices.map((d) => (
              <option key={d.deviceId} value={d.deviceId}>
                {d.name}
              </option>
            ))}
          </select>
        </div>
        <textarea
          value={text}
          placeholder={t("qt.placeholder")}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void send();
            }
          }}
          style={{
            display: "block",
            width: "calc(100% - 40px)",
            margin: "12px 20px 0",
            boxSizing: "border-box",
            minHeight: 96,
            padding: "10px 12px",
            borderRadius: 9,
            border: "1px solid var(--border2)",
            background: "var(--bg)",
            color: "var(--ink)",
            fontFamily: "var(--mono)",
            fontSize: 11.5,
            lineHeight: 1.6,
            outline: "none",
            resize: "none",
          }}
        />
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "12px 20px 16px",
          }}
        >
          <Toggle size="xs" on={clip} onClick={() => setClip((v) => !v)} />
          <span style={{ fontSize: 11.5, color: "var(--muted2)" }}>
            {t("qt.clipToo")}
          </span>
          <div style={{ flex: 1 }} />
          <button
            type="button"
            className={`btn primary${canSend ? "" : " off"}`}
            style={{ padding: "0 16px" }}
            onClick={() => void send()}
          >
            {t("qt.send")}
          </button>
        </div>
        <div className="modal-foot">{t("qt.foot")}</div>
      </div>
    </div>
  );
}
