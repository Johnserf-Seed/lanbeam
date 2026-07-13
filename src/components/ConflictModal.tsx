/** 重名冲突 — name-collision resolver (M6.5). Shown when an incoming request
 *  collides with an existing file AND the conflict policy is "ask" (routed here
 *  from IncomingStack once the SAS is verified). The single choice folds into
 *  one reply_file_request: 保留两者 → accept+rename, 覆盖 → accept+overwrite,
 *  跳过 / 取消接收 → decline. Markup transcribed from「LanBeam 原型 v2」lines
 *  1330-1364. */
import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../bridge/api";
import { errText } from "../lib/sendops";
import { fmtBytes } from "../lib/format";
import {
  notify,
  showToast,
  useOverlays,
  useTransfers,
  useTrust,
} from "../lib/store";

/** The backend resolves collisions per transfer with a single action, so one
 *  choice covers every colliding file. "skip" has no backend disposition — it
 *  declines the whole transfer, the closest honest behavior (per-file skip
 *  isn't part of the reply contract). */
type Choice = "rename" | "overwrite" | "skip";

export default function ConflictModal() {
  const { t } = useTranslation();
  const pending = useOverlays((s) => s.conflict);
  const setConflict = useOverlays((s) => s.setConflict);
  const [sel, setSel] = useState<Choice>("rename");
  const [hover, setHover] = useState<Choice | null>(null);
  const [leaving, setLeaving] = useState(false);

  // Reset the selection each time a fresh collision opens the modal.
  const sessionId = pending?.request.sessionId;
  useEffect(() => {
    setSel("rename");
    setLeaving(false);
  }, [sessionId]);

  // Escape declines (same as 取消接收) — but only while a collision is parked.
  useEffect(() => {
    if (!pending) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") decline();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sessionId]);

  if (!pending) return null;
  const { request, peerName, wantTrust } = pending;
  const names = request.conflicts ?? [];
  const firstName = names[0] ?? "";
  const extra = names.length - 1;
  const newFile = request.files.find((f) => f.name === firstName);
  const newLine =
    newFile && typeof newFile.size === "number"
      ? `${firstName} · ${fmtBytes(newFile.size)}`
      : firstName;

  const close = () => setConflict(null);

  const decline = () => {
    if (leaving) return;
    setLeaving(true);
    api
      .replyFileRequest(request.sessionId, false)
      .catch((e) => showToast(errText(e)));
    close();
    showToast(t("incoming.declineToast", { name: peerName }));
  };

  const accept = (choice: "rename" | "overwrite") => {
    if (leaving) return;
    setLeaving(true);
    // Trust was deferred from the incoming card so declining here grants nothing;
    // a positive choice applies it now (mirrors IncomingStack's accept path).
    if (wantTrust) {
      const trust = useTrust.getState();
      trust.setTrust({ deviceId: request.deviceId, name: peerName }, true);
      const rec = useTrust.getState().records[request.deviceId];
      if (rec && !rec.autoAccept) trust.toggleAuto(request.deviceId);
    }
    const tf = useTransfers.getState();
    tf.acceptMeta(request, peerName);
    api
      .replyFileRequest(request.sessionId, true, choice)
      .catch((e) => showToast(errText(e)));
    close();
    showToast(
      t(wantTrust ? "incoming.acceptTrustToast" : "incoming.acceptToast", {
        name: peerName,
      }),
    );
    notify();
  };

  const confirm = () => (sel === "skip" ? decline() : accept(sel));

  const options: {
    key: Choice;
    label: string;
    sub: string;
    rec?: boolean;
  }[] = [
    {
      key: "rename",
      label: t("conflict.keepBoth"),
      sub: t("conflict.keepBothSub"),
      rec: true,
    },
    {
      key: "overwrite",
      label: t("conflict.overwrite"),
      sub: t("conflict.overwriteSub"),
    },
    { key: "skip", label: t("conflict.skip"), sub: t("conflict.skipSub") },
  ];

  const cmpRow: CSSProperties = {
    display: "flex",
    alignItems: "baseline",
    gap: 8,
    fontSize: 11,
  };

  return (
    <div className="scrim" style={{ zIndex: 62 }}>
      <div className="modal" style={{ width: 412, fontFamily: "var(--font)" }}>
        {/* heading */}
        <div style={{ padding: "18px 20px 0" }}>
          <div
            style={{ fontSize: 14.5, fontWeight: 650, color: "var(--ink2)" }}
          >
            {t("conflict.title", { name: firstName })}
          </div>
          <div style={{ fontSize: 11.5, color: "var(--muted)", marginTop: 2 }}>
            {extra > 0
              ? t("conflict.subMulti", { n: extra })
              : t("conflict.sub")}
          </div>
        </div>

        {/* existing vs incoming */}
        <div
          style={{
            margin: "13px 20px 0",
            background: "var(--sidebar)",
            borderRadius: 10,
            padding: "10px 13px",
            display: "flex",
            flexDirection: "column",
            gap: 7,
          }}
        >
          <div style={cmpRow}>
            <span style={{ width: 44, color: "var(--muted)", flex: "none" }}>
              {t("conflict.existing")}
            </span>
            <span style={{ color: "var(--muted2)" }}>
              {t("conflict.existingLine")}
            </span>
          </div>
          <div style={cmpRow}>
            <span
              style={{
                width: 44,
                color: "var(--accent-ink)",
                fontWeight: 600,
                flex: "none",
              }}
            >
              {t("conflict.incoming")}
            </span>
            <span
              style={{
                color: "var(--ink2)",
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {newLine}
            </span>
          </div>
        </div>

        {/* options */}
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 7,
            padding: "14px 20px 0",
          }}
        >
          {options.map((co) => {
            const active = sel === co.key;
            const borderColor =
              active || hover === co.key ? "var(--accent)" : "var(--border)";
            return (
              // biome-ignore lint/a11y/useSemanticElements: styled radio-option row, not a native button — keeps the custom layout/markup while staying keyboard-operable
              <div
                key={co.key}
                role="button"
                tabIndex={0}
                onClick={() => setSel(co.key)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    if (e.key === " ") e.preventDefault();
                    setSel(co.key);
                  }
                }}
                onMouseEnter={() => setHover(co.key)}
                onMouseLeave={() => setHover(null)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 10,
                  border: `1px solid ${borderColor}`,
                  background: active ? "var(--accent-soft)" : "var(--panel)",
                  borderRadius: 10,
                  padding: "10px 13px",
                  cursor: "pointer",
                  transition: "background .15s ease,border-color .15s ease",
                }}
              >
                <span
                  style={{
                    width: 15,
                    height: 15,
                    borderRadius: "50%",
                    border: active
                      ? "5px solid var(--accent)"
                      : "1.5px solid var(--border2)",
                    background: "var(--panel)",
                    flex: "none",
                    boxSizing: "border-box",
                    transition: "border .15s ease",
                  }}
                />
                <span style={{ flex: 1, minWidth: 0 }}>
                  <span
                    style={{
                      fontSize: 12,
                      fontWeight: 600,
                      color: "var(--ink2)",
                    }}
                  >
                    {co.label}
                  </span>
                  <span
                    style={{
                      display: "block",
                      fontSize: 10.5,
                      color: "var(--muted2)",
                      marginTop: 1,
                    }}
                  >
                    {co.sub}
                  </span>
                </span>
                {co.rec && (
                  <span
                    style={{
                      fontSize: 10,
                      background: "var(--accent-soft)",
                      color: "var(--accent-ink)",
                      borderRadius: 99,
                      padding: "2px 8px",
                      fontWeight: 600,
                      flex: "none",
                    }}
                  >
                    {t("conflict.recommended")}
                  </span>
                )}
              </div>
            );
          })}
        </div>

        {/* footer */}
        <div
          style={{
            display: "flex",
            gap: 9,
            justifyContent: "flex-end",
            padding: "16px 20px 18px",
          }}
        >
          <button
            type="button"
            onClick={decline}
            style={{
              height: 32,
              padding: "0 14px",
              borderRadius: 8,
              border: "1px solid var(--border2)",
              background: "var(--panel)",
              color: "var(--muted2)",
              fontSize: 12,
              fontWeight: 600,
              cursor: "pointer",
              fontFamily: "inherit",
            }}
            onMouseEnter={(e) =>
              (e.currentTarget.style.background = "var(--hover)")
            }
            onMouseLeave={(e) =>
              (e.currentTarget.style.background = "var(--panel)")
            }
          >
            {t("conflict.cancel")}
          </button>
          <button
            type="button"
            onClick={confirm}
            style={{
              height: 32,
              padding: "0 18px",
              borderRadius: 8,
              border: "none",
              background: "var(--accent)",
              color: "var(--accent-fg)",
              fontSize: 12,
              fontWeight: 600,
              cursor: "pointer",
              fontFamily: "inherit",
            }}
            onMouseEnter={(e) =>
              (e.currentTarget.style.filter = "brightness(.94)")
            }
            onMouseLeave={(e) => (e.currentTarget.style.filter = "")}
          >
            {t("conflict.go")}
          </button>
        </div>
      </div>
    </div>
  );
}
