/** 发送流程弹窗 — 4 steps (files → device → confirm → waiting) + step dots.
 *  Styles transcribed from「LanBeam 原型 v2」lines 1007-1148. */
import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import {
  showToast,
  shortFp,
  trustList,
  useData,
  useOverlays,
  usePrefs,
  useTransfers,
  useTrust,
} from "../lib/store";
import type { UITransfer } from "../lib/store";
import { fmtBytes, fmtSas } from "../lib/format";
import { catColors, fileCat } from "../lib/filecat";
import { pickFiles, sendToDevice } from "../lib/sendops";
import { ExtChip, ModalHead } from "./ui";

type WaitState = "waiting" | "ok" | "failed";

/** Square check mark used by the file rows (17px) and device cards (16px). */
function CheckBox({ on, size }: { on: boolean; size: number }) {
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: 5,
        border: `1.5px solid ${on ? "var(--accent)" : "var(--border2)"}`,
        background: on ? "var(--accent)" : "transparent",
        display: "grid",
        placeItems: "center",
        color: "var(--accent-fg)",
        fontSize: size >= 17 ? 11 : 10,
        fontWeight: 700,
        flex: "none",
        boxSizing: "border-box",
      }}
    >
      {on ? "✓" : ""}
    </div>
  );
}

/** 下一步 button style (enabled = accent, else track/muted). */
const nextBtn = (enabled: boolean): CSSProperties => ({
  height: 32,
  padding: "0 16px",
  borderRadius: 8,
  border: "none",
  background: enabled ? "var(--accent)" : "var(--track)",
  color: enabled ? "var(--accent-fg)" : "var(--muted)",
  fontSize: 12.5,
  fontWeight: 600,
  cursor: enabled ? "pointer" : "default",
  fontFamily: "inherit",
  flex: "none",
});

export default function SendModal() {
  const { t } = useTranslation();
  const send = useOverlays((s) => s.send);
  const patchSend = useOverlays((s) => s.patchSend);
  const closeSend = useOverlays((s) => s.closeSend);
  const setShare = useOverlays((s) => s.setShare);
  const setFpAlert = useOverlays((s) => s.setFpAlert);
  const devices = useData((s) => s.devices);
  const records = useTrust((s) => s.records);
  const stripExif = usePrefs((s) => s.stripExif);
  const setPrefs = usePrefs((s) => s.set);
  const transfers = useTransfers((s) => s.transfers);
  /** epoch ms when startSend fired — matches live transfers to pending rows */
  const flowStart = useRef(0);
  // Guard against a slip-off press (mousedown inside, mouseup on scrim) closing
  // the modal and discarding the send config — matches the other four modals.
  const scrimDown = useRef(false);
  /** device card under the pointer — declarative so re-renders keep the
   *  hover border (imperative style writes were clobbered by React). */
  const [hoverId, setHoverId] = useState<string | null>(null);
  const sendStep = send?.step;

  // The grid unmounts without a mouseleave when the step changes — drop the
  // stale hover id so the card doesn't remount pre-hovered.
  useEffect(() => {
    if (sendStep !== "device") setHoverId(null);
  }, [sendStep]);

  /* ── waiting rows derived live from the transfers store ──────────────── */
  const waitRows: {
    deviceId: string;
    name: string;
    sas?: string;
    state: WaitState;
  }[] =
    send?.step === "waiting"
      ? send.pending.map((p) => {
          let best: UITransfer | undefined;
          for (const tr of Object.values(transfers)) {
            if (tr.direction !== "send" || tr.peerId !== p.deviceId) continue;
            if (tr.startedAt < flowStart.current - 1000) continue;
            if (!best || tr.startedAt > best.startedAt) best = tr;
          }
          const state: WaitState = p.failed
            ? "failed"
            : best?.status === "error"
              ? "failed"
              : best?.started
                ? "ok"
                : "waiting";
          return { deviceId: p.deviceId, name: p.name, sas: best?.sas, state };
        })
      : [];
  const allOk = waitRows.length > 0 && waitRows.every((r) => r.state === "ok");
  const firstWaitName = waitRows[0]?.name ?? "";
  const waitCount = waitRows.length;

  // Every pending device confirmed → close and report.
  useEffect(() => {
    if (!allOk) return;
    closeSend();
    showToast(
      waitCount === 1
        ? t("send.sentOneToast", { name: firstWaitName })
        : t("send.sentAllToast"),
    );
  }, [allOk, waitCount, firstWaitName, closeSend, t]);

  if (!send) return null;

  const step = send.step;
  const selFiles = send.pool.filter((f) => send.sel.includes(f.path ?? f.name));
  const totalBytes = selFiles.reduce((a, f) => a + (f.size ?? 0), 0);
  // Picked/dropped files carry no size in Tauri mode — show "—" instead of a
  // fabricated "0 KB" whenever any selected file's size is unknown.
  const sizeKnown =
    selFiles.length > 0 && selFiles.every((f) => f.size != null);
  const totalStr = sizeKnown ? fmtBytes(totalBytes) : "—";
  const targets = devices.filter((d) => send.deviceIds.includes(d.deviceId));

  // Live devices whose fingerprint no longer matches a remembered record.
  const tl = trustList(devices, records);
  const oldIdByNew: Record<string, string> = {};
  for (const x of tl)
    if (x.fpChanged) oldIdByNew[x.fpChanged.newDeviceId] = x.deviceId;

  /* ── header title / sub per step ──────────────────────────────────────── */
  const title =
    step === "files"
      ? t("send.titleFiles")
      : step === "device"
        ? t("send.titleDevice")
        : step === "confirm"
          ? t("send.titleConfirm")
          : t("send.titleWaiting");
  const sub =
    step === "files"
      ? targets.length === 1
        ? t("send.subFilesTo", { name: targets[0].name })
        : targets.length > 1
          ? t("send.subFilesToN", { n: targets.length })
          : t("send.subFilesPick")
      : step === "device"
        ? t("send.subDevice", { n: selFiles.length, size: totalStr })
        : step === "confirm"
          ? t("send.subConfirm")
          : t("send.subWaiting");

  /* ── step dots (hidden while waiting) ─────────────────────────────────── */
  const stepIdx = { files: 0, device: 1, confirm: 2, waiting: 2 }[step];
  const dots = [
    { label: t("send.step1"), idx: 0 },
    ...(send.preset ? [] : [{ label: t("send.step2"), idx: 1 }]),
    { label: t("send.step3", { n: send.preset ? 2 : 3 }), idx: 2 },
  ];

  /* ── actions ──────────────────────────────────────────────────────────── */
  const onClose = () => {
    if (step === "waiting") showToast(t("send.canceledToast"));
    closeSend();
  };

  const toggleSel = (key: string) =>
    patchSend({
      sel: send.sel.includes(key)
        ? send.sel.filter((x) => x !== key)
        : [...send.sel, key],
    });

  const pick = async () => {
    const picked = await pickFiles(t("send.titleFiles"));
    if (!picked.length) return;
    const cur = useOverlays.getState().send;
    if (!cur) return;
    const known = new Set(cur.pool.map((f) => f.path ?? f.name));
    const fresh = picked.filter((f) => !known.has(f.path ?? f.name));
    const sel = [...cur.sel];
    for (const f of picked) {
      const k = f.path ?? f.name;
      if (!sel.includes(k)) sel.push(k);
    }
    patchSend({ pool: [...cur.pool, ...fresh], sel });
  };

  const sendNext = () => {
    if (!selFiles.length) return;
    patchSend({
      step: send.preset && send.deviceIds.length ? "confirm" : "device",
    });
  };

  const toggleDevice = (id: string) =>
    patchSend({
      deviceIds: send.deviceIds.includes(id)
        ? send.deviceIds.filter((x) => x !== id)
        : [...send.deviceIds, id],
    });

  const startSend = () => {
    if (!selFiles.length || !targets.length) return;
    let ok = false;
    // Pass the confirm dialog's live "抹除图片元数据" choice (M9.1) explicitly,
    // rather than letting sendToDevice fall back to the persisted default.
    for (const d of targets) ok = sendToDevice(d, selFiles, stripExif) || ok;
    if (!ok) {
      // No real paths / no backend — sendToDevice already toasted honestly.
      closeSend();
      return;
    }
    const trusted = targets.filter((d) => records[d.deviceId]?.trusted);
    const untrusted = targets.filter((d) => !records[d.deviceId]?.trusted);
    if (!untrusted.length) {
      closeSend();
      showToast(
        trusted.length > 1
          ? t("send.startTrustedToast", { n: trusted.length })
          : t("devices.startSendToast", {
              n: selFiles.length,
              name: trusted[0].name,
            }),
      );
      return;
    }
    flowStart.current = Date.now();
    patchSend({
      step: "waiting",
      pending: untrusted.map((d) => ({
        deviceId: d.deviceId,
        name: d.name,
        ok: false,
      })),
      startedTrusted: trusted.length,
    });
  };

  /* ── render ───────────────────────────────────────────────────────────── */
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc
    // biome-ignore lint/a11y/useKeyWithClickEvents: same
    <div
      className="scrim"
      style={{ zIndex: 50 }}
      onMouseDown={(e) => {
        scrimDown.current = e.target === e.currentTarget;
      }}
      onClick={() => {
        if (scrimDown.current) onClose();
      }}
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: onClick only stops propagation so clicks inside the modal don't reach the backdrop — not an interactive control */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
      <div
        className="modal"
        style={{ width: 500, fontFamily: "var(--font)" }}
        onClick={(e) => e.stopPropagation()}
      >
        <ModalHead
          title={title}
          sub={sub}
          onClose={onClose}
          pad="18px 20px 12px"
        />

        {step !== "waiting" && (
          <div style={{ display: "flex", gap: 6, padding: "0 20px 14px" }}>
            {dots.map((d) => {
              const on = d.idx === stepIdx;
              const done = d.idx < stepIdx;
              return (
                <span
                  key={d.idx}
                  style={{
                    fontSize: 10.5,
                    fontWeight: 600,
                    padding: "3px 9px",
                    borderRadius: 99,
                    background: on ? "var(--accent-soft)" : "transparent",
                    color: on
                      ? "var(--accent-ink)"
                      : done
                        ? "var(--muted2)"
                        : "var(--muted)",
                  }}
                >
                  {d.label}
                </span>
              );
            })}
          </div>
        )}

        {/* ── step 1 · files ─────────────────────────────────────────────── */}
        {step === "files" && (
          <div
            style={{
              borderTop: "1px solid var(--border)",
              animation: "lbFade .18s ease",
            }}
          >
            <div className="scroll-y" style={{ maxHeight: 305 }}>
              {send.pool.map((f) => {
                const key = f.path ?? f.name;
                const on = send.sel.includes(key);
                return (
                  // biome-ignore lint/a11y/useSemanticElements: styled file row, not a native button — keeps the custom layout/markup while staying keyboard-operable
                  <div
                    key={key}
                    className="hover-row"
                    role="button"
                    tabIndex={0}
                    onClick={() => toggleSel(key)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        if (e.key === " ") e.preventDefault();
                        toggleSel(key);
                      }
                    }}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 12,
                      padding: "10px 20px",
                      cursor: "pointer",
                    }}
                  >
                    <CheckBox on={on} size={17} />
                    <ExtChip ext={f.ext} size={30} fontSize={8.5} />
                    <div
                      style={{
                        flex: 1,
                        minWidth: 0,
                        fontSize: 12.5,
                        fontWeight: 600,
                        color: "var(--ink2)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {f.name}
                    </div>
                    <span
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 11,
                        color: "var(--muted)",
                        flex: "none",
                      }}
                    >
                      {f.size != null ? fmtBytes(f.size) : "—"}
                    </span>
                  </div>
                );
              })}
            </div>
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                padding: "14px 20px 18px",
                borderTop: "1px solid var(--border)",
              }}
            >
              <button
                type="button"
                className="btn ink"
                onClick={() => void pick()}
              >
                {t("send.pickFromDisk")}
              </button>
              <div
                style={{
                  flex: 1,
                  textAlign: "right",
                  fontSize: 11.5,
                  color: "var(--muted)",
                }}
              >
                {selFiles.length
                  ? t("send.selLine", {
                      n: selFiles.length,
                      size: totalStr,
                    })
                  : t("send.selNone")}
              </div>
              <button
                type="button"
                style={nextBtn(selFiles.length > 0)}
                onClick={sendNext}
              >
                {t("common.next")}
              </button>
            </div>
          </div>
        )}

        {/* ── step 2 · device ────────────────────────────────────────────── */}
        {step === "device" && (
          <div
            style={{
              borderTop: "1px solid var(--border)",
              animation: "lbFade .18s ease",
            }}
          >
            <div className="scroll-y" style={{ maxHeight: 305 }}>
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: "1fr 1fr",
                  gap: 10,
                  padding: "14px 20px 4px",
                }}
              >
                {devices.map((d) => {
                  const on = send.deviceIds.includes(d.deviceId);
                  const changed = d.deviceId in oldIdByNew;
                  const trusted = records[d.deviceId]?.trusted ?? false;
                  return (
                    // biome-ignore lint/a11y/useSemanticElements: styled device card, not a native button — keeps the custom layout/markup while staying keyboard-operable
                    <div
                      key={d.deviceId}
                      title={d.address}
                      role="button"
                      tabIndex={0}
                      onClick={() => {
                        if (changed) {
                          setFpAlert({ deviceId: oldIdByNew[d.deviceId] });
                          return;
                        }
                        toggleDevice(d.deviceId);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          if (e.key === " ") e.preventDefault();
                          if (changed) {
                            setFpAlert({ deviceId: oldIdByNew[d.deviceId] });
                            return;
                          }
                          toggleDevice(d.deviceId);
                        }
                      }}
                      onMouseEnter={() => setHoverId(d.deviceId)}
                      onMouseLeave={() => setHoverId(null)}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 10,
                        padding: "11px 13px",
                        border: `1px solid ${
                          on || (hoverId === d.deviceId && !changed)
                            ? "var(--accent)"
                            : "var(--border2)"
                        }`,
                        background: on ? "var(--accent-soft)" : "transparent",
                        borderRadius: 12,
                        cursor: changed ? "not-allowed" : "pointer",
                        opacity: changed ? 0.45 : 1,
                        transition:
                          "background .15s ease,border-color .15s ease",
                      }}
                    >
                      <CheckBox on={on} size={16} />
                      <div style={{ minWidth: 0, flex: 1 }}>
                        <div
                          style={{
                            display: "flex",
                            alignItems: "center",
                            gap: 7,
                          }}
                        >
                          <span
                            style={{
                              width: 8,
                              height: 8,
                              borderRadius: "50%",
                              background: "var(--dot-live)",
                              boxShadow: "0 0 12px var(--glow)",
                              flex: "none",
                              boxSizing: "border-box",
                            }}
                          />
                          <span
                            style={{
                              fontSize: 12.5,
                              fontWeight: 600,
                              color: "var(--ink2)",
                              whiteSpace: "nowrap",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                            }}
                          >
                            {d.name}
                          </span>
                        </div>
                        <div
                          style={{
                            fontSize: 10.5,
                            color: changed
                              ? "var(--danger)"
                              : trusted
                                ? "var(--success)"
                                : "var(--muted2)",
                            fontWeight: 600,
                            marginTop: 2,
                            whiteSpace: "nowrap",
                          }}
                        >
                          {changed
                            ? t("send.noteFpChanged")
                            : trusted
                              ? t("send.noteTrusted")
                              : t("send.noteAsk")}
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
            {devices.length === 0 && (
              <div
                style={{
                  padding: "10px 20px 4px",
                  textAlign: "center",
                  fontSize: 12,
                  color: "var(--muted)",
                }}
              >
                {t("send.devEmpty")}
              </div>
            )}
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                padding: "12px 20px 18px",
                borderTop: "1px solid var(--border)",
                marginTop: 10,
              }}
            >
              {/* biome-ignore lint/a11y/useSemanticElements: styled inline link, not a native button — keeps the custom layout/markup while staying keyboard-operable */}
              <span
                role="button"
                tabIndex={0}
                onClick={() => setShare(true)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    if (e.key === " ") e.preventDefault();
                    setShare(true);
                  }
                }}
                style={{
                  fontSize: 11.5,
                  color: "var(--accent-ink)",
                  cursor: "pointer",
                  whiteSpace: "nowrap",
                }}
              >
                {t("send.browserRecv")}
              </span>
              <div
                style={{
                  flex: 1,
                  textAlign: "right",
                  fontSize: 11.5,
                  color: "var(--muted)",
                  whiteSpace: "nowrap",
                }}
              >
                {send.deviceIds.length
                  ? t("send.devLine", { n: send.deviceIds.length })
                  : t("send.devNone")}
              </div>
              <button
                type="button"
                style={nextBtn(send.deviceIds.length > 0)}
                onClick={() => {
                  if (send.deviceIds.length) patchSend({ step: "confirm" });
                }}
              >
                {t("common.next")}
              </button>
            </div>
          </div>
        )}

        {/* ── step 3 · confirm ───────────────────────────────────────────── */}
        {step === "confirm" && (
          <div
            style={{
              borderTop: "1px solid var(--border)",
              padding: "16px 20px 20px",
              display: "flex",
              flexDirection: "column",
              gap: 13,
              animation: "lbFade .18s ease",
            }}
          >
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              <div
                className="scroll-y"
                style={{
                  maxHeight: 180,
                  display: "flex",
                  flexDirection: "column",
                  gap: 6,
                }}
              >
                {targets.map((d) => {
                  const trusted = records[d.deviceId]?.trusted ?? false;
                  return (
                    <div
                      key={d.deviceId}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 11,
                        background: "var(--accent-soft)",
                        borderRadius: 12,
                        padding: "10px 14px",
                      }}
                    >
                      <div
                        style={{
                          width: 10,
                          height: 10,
                          borderRadius: "50%",
                          background: "var(--dot-live)",
                          boxShadow: "0 0 10px var(--glow)",
                          flex: "none",
                        }}
                      />
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          style={{
                            fontSize: 12.5,
                            fontWeight: 650,
                            color: "var(--ink2)",
                            whiteSpace: "nowrap",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                          }}
                        >
                          {d.name}
                        </div>
                        <div
                          style={{
                            fontFamily: "var(--mono)",
                            fontSize: 10.5,
                            color: "var(--muted)",
                          }}
                        >
                          {shortFp(d.deviceId)} · {d.address}
                        </div>
                      </div>
                      <span
                        style={{
                          fontSize: 10.5,
                          fontWeight: 600,
                          color: trusted ? "var(--success)" : "var(--muted2)",
                          flex: "none",
                        }}
                      >
                        {trusted ? t("send.noteTrusted") : t("send.noteAskSas")}
                      </span>
                    </div>
                  );
                })}
              </div>
              <div style={{ display: "flex", justifyContent: "flex-end" }}>
                {/* biome-ignore lint/a11y/useSemanticElements: styled inline link, not a native button — keeps the custom layout/markup while staying keyboard-operable */}
                <span
                  role="button"
                  tabIndex={0}
                  onClick={() => patchSend({ step: "device", preset: false })}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      if (e.key === " ") e.preventDefault();
                      patchSend({ step: "device", preset: false });
                    }
                  }}
                  style={{
                    fontSize: 11.5,
                    color: "var(--accent-ink)",
                    cursor: "pointer",
                  }}
                >
                  {t("send.changeDevice")}
                </span>
              </div>
            </div>

            <div
              style={{
                border: "1px solid var(--border)",
                borderRadius: 12,
                overflow: "hidden",
              }}
            >
              {selFiles.slice(0, 4).map((f) => {
                const [fg, bg] = catColors(f.ext);
                return (
                  <div
                    key={f.path ?? f.name}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 10,
                      padding: "9px 14px",
                      borderBottom: "1px solid var(--border)",
                    }}
                  >
                    <span
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 8.5,
                        fontWeight: 600,
                        color: fg,
                        background: bg,
                        borderRadius: 6,
                        padding: "4px 6px",
                        flex: "none",
                      }}
                    >
                      {f.ext}
                    </span>
                    <span
                      style={{
                        flex: 1,
                        fontSize: 12,
                        fontWeight: 600,
                        color: "var(--ink2)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {f.name}
                    </span>
                    <span
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 10.5,
                        color: "var(--muted)",
                        flex: "none",
                      }}
                    >
                      {f.size != null ? fmtBytes(f.size) : "—"}
                    </span>
                  </div>
                );
              })}
              {selFiles.length > 4 && (
                <div
                  style={{
                    padding: "8px 14px",
                    fontSize: 11,
                    color: "var(--muted)",
                  }}
                >
                  {t("send.moreFiles", { n: selFiles.length - 4 })}
                </div>
              )}
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  padding: "10px 14px",
                  background: "var(--sidebar)",
                }}
              >
                <span style={{ fontSize: 11.5, color: "var(--muted2)" }}>
                  {t("send.totalFiles", { n: selFiles.length })}
                </span>
                <span
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 11.5,
                    fontWeight: 600,
                    color: "var(--ink2)",
                  }}
                >
                  {totalStr}
                </span>
              </div>
            </div>

            {selFiles.some((f) => fileCat(f.ext) === "img") && (
              <label
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  fontSize: 12,
                  fontWeight: 600,
                  color: "var(--ink2)",
                  cursor: "pointer",
                }}
              >
                <input
                  type="checkbox"
                  checked={stripExif}
                  onChange={(e) => setPrefs({ stripExif: e.target.checked })}
                  style={{ accentColor: "var(--accent)", margin: 0 }}
                />
                {t("send.stripExif")}
                <span
                  style={{
                    fontSize: 10.5,
                    fontWeight: 400,
                    color: "var(--muted)",
                  }}
                >
                  {t("send.stripExifSub")}
                </span>
              </label>
            )}

            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 10.5,
                color: "var(--muted)",
                textAlign: "center",
              }}
            >
              {t("send.e2eLine")}
            </div>

            <div
              style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}
            >
              <button
                type="button"
                className="btn ink"
                style={{ height: 34, padding: "0 15px", fontSize: 12.5 }}
                onClick={() =>
                  patchSend({ step: send.preset ? "files" : "device" })
                }
              >
                {t("common.back")}
              </button>
              <button
                type="button"
                className="btn primary"
                style={{ height: 34, padding: "0 18px" }}
                onClick={startSend}
              >
                <span style={{ fontSize: 14, lineHeight: 1 }}>↑</span>
                {targets.length > 1
                  ? t("send.startN", { n: targets.length })
                  : t("send.start")}
              </button>
            </div>
          </div>
        )}

        {/* ── step 4 · waiting ───────────────────────────────────────────── */}
        {step === "waiting" && (
          <div
            style={{
              borderTop: "1px solid var(--border)",
              padding: "16px 20px 20px",
              display: "flex",
              flexDirection: "column",
              gap: 12,
              animation: "lbFade .18s ease",
            }}
          >
            {send.startedTrusted > 0 && (
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  background: "var(--success-soft)",
                  borderRadius: 10,
                  padding: "8px 12px",
                  fontSize: 11.5,
                  fontWeight: 600,
                  color: "var(--success)",
                }}
              >
                ✓ {t("send.waitingNote", { n: send.startedTrusted })}
              </div>
            )}
            <div
              className="scroll-y"
              style={{
                maxHeight: 240,
                display: "flex",
                flexDirection: "column",
                gap: 7,
              }}
            >
              {waitRows.map((p) => (
                <div
                  key={p.deviceId}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 11,
                    border: "1px solid var(--border)",
                    borderRadius: 12,
                    padding: "10px 14px",
                  }}
                >
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: "50%",
                      background:
                        p.state === "ok"
                          ? "var(--success)"
                          : p.state === "failed"
                            ? "var(--danger)"
                            : "var(--accent)",
                      animation:
                        p.state === "waiting"
                          ? "lbBlink 1.4s ease-in-out infinite"
                          : "none",
                      flex: "none",
                    }}
                  />
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div
                      style={{
                        fontSize: 12.5,
                        fontWeight: 600,
                        color: "var(--ink2)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {p.name}
                    </div>
                    <div
                      style={{
                        fontSize: 10.5,
                        color:
                          p.state === "ok"
                            ? "var(--success)"
                            : p.state === "failed"
                              ? "var(--danger)"
                              : "var(--muted)",
                        fontWeight: 600,
                        marginTop: 1,
                      }}
                    >
                      {p.state === "ok"
                        ? t("send.okState")
                        : p.state === "failed"
                          ? t("send.failedState")
                          : t("send.waitingState")}
                    </div>
                  </div>
                  <div style={{ textAlign: "right", flex: "none" }}>
                    <div
                      style={{
                        fontSize: 9.5,
                        color: "var(--muted)",
                        letterSpacing: ".04em",
                      }}
                    >
                      {t("send.sasLabel")}
                    </div>
                    <div
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 15,
                        fontWeight: 600,
                        color: "var(--accent-ink)",
                        letterSpacing: ".05em",
                        marginTop: 1,
                      }}
                    >
                      {fmtSas(p.sas) || "···"}
                    </div>
                  </div>
                </div>
              ))}
            </div>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 10.5,
                color: "var(--muted)",
                textAlign: "center",
              }}
            >
              {t("send.sasNote")}
            </div>
            <div style={{ display: "flex", justifyContent: "center" }}>
              <button
                type="button"
                className="btn"
                style={{ height: 32, padding: "0 18px", fontSize: 12.5 }}
                onClick={onClose}
              >
                {t("send.cancelWaiting")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
