import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { showToast, useData, useOverlays } from "../lib/store";
import { fmtBytes } from "../lib/format";
import { isMac } from "../lib/platform";
import { MAX_TEXT_BYTES, quickTextSize } from "../lib/quickText";
import { errText, sendTextTracked } from "../lib/sendops";
import { useEscape } from "../lib/useEscape";
import { MODAL_W, ModalHead, Toggle } from "./ui";

/** An action that reads as a link inside a line of text. */
const linkBtn = {
  border: "none",
  background: "none",
  padding: 0,
  font: "inherit",
  color: "var(--accent-ink)",
  cursor: "pointer",
} as const;

/** ⌁ 快传文本 — sends a short text/link over the encrypted channel for real
 *  (send_text, M7.3). The clipboard toggle is a REQUEST to the receiver; their
 *  own clipboard-sharing consent decides whether it actually lands there. */
export default function QuickTextModal() {
  const { t } = useTranslation();
  const qtOpen = useOverlays((s) => s.qtOpen);
  const setQt = useOverlays((s) => s.setQt);
  const setPair = useOverlays((s) => s.setPair);
  const devices = useData((s) => s.devices);

  const [text, setText] = useState("");
  const [clip, setClip] = useState(true);
  // The device the text goes to, pinned once the picker shows one (M26). The
  // name is kept so a device that drops off the list can still be named.
  const [target, setTarget] = useState<{ id: string; name: string } | null>(
    null,
  );
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
      setTarget(null);
      setSending(false);
    }
  }, [qtOpen]);

  // Pin the first device as soon as there is one to show — so that the device
  // on screen is the device the text goes to. A target that vanished from the
  // list used to be silently replaced by whichever device sorted first, and
  // ⌘⏎ sent the text there (M26).
  useEffect(() => {
    if (qtOpen && !target && devices[0])
      setTarget({ id: devices[0].deviceId, name: devices[0].name });
  }, [qtOpen, target, devices]);

  useEscape(qtOpen, () => setQt(false));

  if (!qtOpen) return null;
  const close = () => setQt(false);
  const live = target
    ? devices.find((d) => d.deviceId === target.id)
    : undefined;
  const targetId = target?.id ?? "";
  const targetName = live?.name ?? target?.name ?? "";
  // Gone offline: say so and wait — never send somewhere else instead.
  const offline = !!target && !live;
  // Measured on the trimmed text — that is what goes out. Over the cap, the send
  // is refused here; the backend would only answer TextTooLong anyway.
  const size = quickTextSize(text.trim());
  const canSend = !!text.trim() && size.fits && !!live && !sending;

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
        style={{ width: MODAL_W.narrow, fontFamily: "var(--font)" }}
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
            onChange={(e) => {
              const d = devices.find((x) => x.deviceId === e.target.value);
              if (d) setTarget({ id: d.deviceId, name: d.name });
            }}
          >
            {devices.map((d) => (
              <option key={d.deviceId} value={d.deviceId}>
                {d.name}
              </option>
            ))}
            {offline && (
              <option value={targetId} disabled>
                {t("qt.offlineOption", { name: targetName })}
              </option>
            )}
            {!devices.length && !offline && (
              <option value="">{t("qt.noDevices")}</option>
            )}
          </select>
        </div>
        {!devices.length && !offline && (
          // An empty, greyed-out list said nothing about why or what to do.
          <div
            style={{
              padding: "6px 20px 0 20px",
              fontSize: 11.5,
              color: "var(--muted)",
            }}
          >
            {t("qt.noDevicesHint")}
            {" · "}
            <button
              type="button"
              style={linkBtn}
              onClick={() => {
                setQt(false);
                setPair(true);
              }}
            >
              {t("qt.pair")}
            </button>
          </div>
        )}
        {offline && (
          <div
            style={{
              padding: "6px 20px 0 20px",
              fontSize: 11.5,
              color: "var(--danger)",
            }}
          >
            {t("qt.targetOffline", { name: targetName })}
          </div>
        )}
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
        {text.trim() && (
          <div
            style={{
              margin: "6px 20px 0",
              textAlign: "right",
              fontSize: 11,
              color: size.fits ? "var(--muted2)" : "var(--danger)",
            }}
          >
            {!size.fits && `${t("qt.tooLong")} · `}
            {fmtBytes(size.bytes)} / {fmtBytes(MAX_TEXT_BYTES)}
          </div>
        )}
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
            {t("qt.send")}{" "}
            {/* The chord as THIS OS spells it — the handler above takes either
                modifier, but a Windows user shown ⌘⏎ has no ⌘ key to press.
                Hidden from the accessible name: "Send", not "Send ⌘⏎". */}
            <span aria-hidden="true">{isMac() ? "⌘⏎" : "Ctrl+Enter"}</span>
          </button>
        </div>
        <div className="modal-foot">{t("qt.foot")}</div>
      </div>
    </div>
  );
}
