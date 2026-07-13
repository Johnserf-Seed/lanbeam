/** 设备 — device radar: sweep, live nodes, transfer beams, empty checklist. */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import * as api from "../bridge/api";
import {
  showToast,
  transferList,
  trustList,
  useData,
  useOverlays,
  usePrefs,
  useRecents,
  useTransfers,
  useTrust,
  visibilityOf,
  type UITransfer,
} from "../lib/store";
import { estChipW, etaClock, fmtBytes } from "../lib/format";
import { errText } from "../lib/sendops";
import { radarBasePos, resolveChips, type ChipNode } from "../lib/radar";
import { ExtChip } from "../components/ui";

const mbps = (bps: number): string => `${(bps / 1048576).toFixed(1)} MB/s`;

type NodePos = { x: number; y: number; mirror: boolean };

type Beam = {
  key: string;
  x: number;
  y: number;
  w: number;
  rot: string;
  grad: string;
  dash: string;
  chipX: number;
  chipY: number;
  chipBg: string;
  pctStr: string;
};

export default function DevicesPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const settings = useData((s) => s.settings);
  const devices = useData((s) => s.devices);
  const setDevices = useData((s) => s.setDevices);
  const firstSeen = useData((s) => s.firstSeen);
  const ghostUntil = usePrefs((s) => s.ghostUntil);
  const records = useTrust((s) => s.records);
  const transfersMap = useTransfers((s) => s.transfers);
  const recents = useRecents((s) => s.items);
  const dragDevice = useOverlays((s) => s.dragDevice);
  const openSend = useOverlays((s) => s.openSend);
  const setFpAlert = useOverlays((s) => s.setFpAlert);
  const setPair = useOverlays((s) => s.setPair);

  const [ipTry, setIpTry] = useState("");
  const [ipBusy, setIpBusy] = useState(false);

  /** IP 直连 (M7.2): dial an ip[:port] (or a scanned link), then re-query the
   *  device list so the manually-added peer shows on the radar. */
  const tryDirect = async () => {
    const addr = ipTry.trim();
    if (!addr || ipBusy) return;
    // Reject obvious non-addresses (a bare word/number — no "." and no "://")
    // client-side with a localized hint, instead of a raw backend error.
    if (!addr.includes(".") && !addr.includes("://")) {
      showToast(t("devices.badAddr"));
      return;
    }
    setIpBusy(true);
    try {
      const r = await api.connectByAddr(addr);
      // connect_by_addr adds the peer to the manual table; it surfaces through
      // list_discovered_devices (merged), not the discovery event — so re-query.
      const list = await api.listDiscoveredDevices();
      setDevices(list);
      setIpTry("");
      showToast(t("devices.ipConnected", { name: r.name }));
    } catch (e) {
      showToast(t("devices.ipFailed", { err: errText(e) }));
    } finally {
      setIpBusy(false);
    }
  };

  /* ── clock tick so「刚刚发现」expires (60 s window) ─────────────────── */
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 10_000);
    return () => clearInterval(id);
  }, []);

  /* ── radar scale via ResizeObserver ─────────────────────────────────── */
  const [rk, setRk] = useState(1);
  const roRef = useRef<ResizeObserver | null>(null);
  const setRadarWrap = useCallback((el: HTMLDivElement | null) => {
    roRef.current?.disconnect();
    roRef.current = null;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      if (!r.width || !r.height) return;
      const sc = Math.max(
        0.55,
        Math.min(1, (r.height - 8) / 500, (r.width - 16) / 690),
      );
      setRk((p) => (Math.abs(sc - p) > 0.004 ? sc : p));
    };
    if (typeof ResizeObserver !== "undefined") {
      roRef.current = new ResizeObserver(measure);
      roRef.current.observe(el);
    }
    measure();
  }, []);

  const rW = Math.round(690 * rk);
  const rH = Math.round(500 * rk);
  const rcX = Math.round(rW / 2);
  const rcY = Math.round(rH / 2);
  const rr = (v: number) => Math.round(v * rk);

  /* ── sweep: rAF-driven conic gradient, 360° / 7 s, ~30 fps ──────────── */
  const sweepRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    let raf = 0;
    let last = 0;
    const loop = (ts: number) => {
      raf = requestAnimationFrame(loop);
      if (ts - last < 33) return;
      last = ts;
      const el = sweepRef.current;
      if (!el) return;
      const a = (((ts % 7000) / 7000) * 360).toFixed(1);
      el.style.background = `conic-gradient(from ${a}deg,var(--sweep),rgba(0,0,0,0) 70deg)`;
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);

  /* ── trust overlay: renamed labels + fingerprint-mismatch mapping ───── */
  const tl = useMemo(() => trustList(devices, records), [devices, records]);
  const nameById = useMemo(() => {
    const m: Record<string, string> = {};
    for (const r of tl) if (r.online) m[r.deviceId] = r.name;
    return m;
  }, [tl]);
  /** live deviceId → old (remembered) record id whose fingerprint changed */
  const mismatchById = useMemo(() => {
    const m: Record<string, string> = {};
    for (const r of tl)
      if (r.fpChanged) m[r.fpChanged.newDeviceId] = r.deviceId;
    return m;
  }, [tl]);

  /* ── active transfers ───────────────────────────────────────────────── */
  const running = useMemo(
    () => transferList(transfersMap).filter((x) => x.status === "active"),
    [transfersMap],
  );
  const runningByPeer = useMemo(() => {
    const m: Record<string, UITransfer> = {};
    for (const x of running) if (x.peerId && !m[x.peerId]) m[x.peerId] = x;
    return m;
  }, [running]);

  /* ── node placement: deterministic base + collision resolve ─────────── */
  const nodePos = useMemo(() => {
    const list: ChipNode[] = devices.map((d, i) => {
      const bp = radarBasePos(d.deviceId, i);
      const x = rcX + Math.round((bp.x - 345) * rk);
      const y = rcY + Math.round((bp.y - 250) * rk);
      const name = nameById[d.deviceId] ?? d.name;
      return {
        id: d.deviceId,
        x,
        y,
        mirror: x < rcX,
        inside: false,
        w: estChipW(name, d.address) + 12,
      };
    });
    resolveChips(list, {
      tcX: rcX,
      tcY: rcY,
      R2: 64,
      bw: rW,
      bh: rH,
      drag: null,
      anchor: 23,
      rowH: 52,
    });
    const map: Record<string, NodePos> = {};
    for (const n of list)
      map[n.id] = { x: Math.round(n.x), y: Math.round(n.y), mirror: n.mirror };
    return map;
  }, [devices, nameById, rk, rcX, rcY, rW, rH]);

  /* ── beams: center hub ↔ node, direction-colored ────────────────────── */
  const beams: Beam[] = [];
  for (const x of running) {
    const rp = x.peerId ? nodePos[x.peerId] : undefined;
    if (!rp) continue;
    const color =
      x.direction === "receive" ? "var(--success)" : "var(--accent)";
    const dx = rp.x - rcX;
    const dy = rp.y - rcY;
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len;
    const uy = dy / len;
    const sx = rcX + ux * 38;
    const sy = rcY + uy * 38;
    const ex = rp.x - ux * 20;
    const ey = rp.y - uy * 20;
    const from: [number, number] =
      x.direction === "receive" ? [ex, ey] : [sx, sy];
    const to: [number, number] =
      x.direction === "receive" ? [sx, sy] : [ex, ey];
    const w = Math.hypot(to[0] - from[0], to[1] - from[1]);
    const rot = (Math.atan2(to[1] - from[1], to[0] - from[0]) * 180) / Math.PI;
    beams.push({
      key: x.sessionId,
      x: Math.round(from[0]),
      y: Math.round(from[1]),
      w: Math.round(w),
      rot: rot.toFixed(2),
      grad: `linear-gradient(90deg,rgba(0,0,0,0),${color})`,
      dash: `repeating-linear-gradient(90deg,${color} 0 5px,rgba(0,0,0,0) 5px 13px)`,
      chipX: Math.round(from[0] + (to[0] - from[0]) * 0.55),
      chipY: Math.round(from[1] + (to[1] - from[1]) * 0.55),
      chipBg: color,
      pctStr: `${Math.round(x.percent)}%`,
    });
  }

  /* ── strip / aggregate values ───────────────────────────────────────── */
  const strip = running.length === 1 ? running[0] : null;
  const aggSize = running.reduce((a, x) => a + x.totalSize, 0);
  const aggDone = running.reduce(
    (a, x) => a + (x.totalSize * Math.min(x.percent, 100)) / 100,
    0,
  );
  const aggPct = aggSize ? Math.min(100, (aggDone / aggSize) * 100) : 0;
  const aggSpeedBps = running.reduce((a, x) => a + x.speedBps, 0);
  const aggEtaClock = etaClock(aggSize, aggPct, aggSpeedBps);
  const aggExts = Array.from(
    new Set(running.map((x) => x.ext ?? "FILE")),
  ).slice(0, 3);
  const nOut = running.filter((x) => x.direction === "send").length;
  const nIn = running.filter((x) => x.direction === "receive").length;

  const vis = visibilityOf(settings, ghostUntil, now);
  const deviceName = settings?.deviceName ?? "";

  const barBase: CSSProperties = {
    margin: "0 24px",
    background: "var(--panel)",
    border: "1px solid var(--border)",
    borderRadius: 12,
    padding: "11px 16px",
    display: "flex",
    alignItems: "center",
    gap: 14,
    animation: "lbFade .18s ease",
  };

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
        animation: "lbFade .18s ease",
      }}
    >
      {/* ── radar cell ─────────────────────────────────────────────────── */}
      <div
        ref={setRadarWrap}
        style={{
          flex: 1,
          display: "grid",
          placeItems: "center",
          minHeight: 0,
          overflow: "hidden",
        }}
      >
        <div style={{ position: "relative", width: rW, height: rH }}>
          <div
            style={{
              position: "absolute",
              left: rcX,
              top: rcY,
              width: rr(280) * 2,
              height: rr(280) * 2,
              margin: `${-rr(280)}px 0 0 ${-rr(280)}px`,
              borderRadius: "50%",
              background:
                "radial-gradient(circle,var(--radial) 0%,rgba(0,0,0,0) 62%)",
            }}
          />
          {(
            [
              [270, "var(--ring-c)"],
              [190, "var(--ring-b)"],
              [110, "var(--ring-a)"],
            ] as [number, string][]
          ).map(([radius, color]) => (
            <div
              key={radius}
              style={{
                position: "absolute",
                left: rcX,
                top: rcY,
                width: rr(radius) * 2,
                height: rr(radius) * 2,
                margin: `${-rr(radius)}px 0 0 ${-rr(radius)}px`,
                border: `1px solid ${color}`,
                borderRadius: "50%",
              }}
            />
          ))}
          <div
            ref={sweepRef}
            style={{
              position: "absolute",
              left: rcX,
              top: rcY,
              width: rr(270) * 2,
              height: rr(270) * 2,
              margin: `${-rr(270)}px 0 0 ${-rr(270)}px`,
              borderRadius: "50%",
            }}
          />
          <div
            style={{
              position: "absolute",
              left: rcX,
              top: rcY,
              borderRadius: "50%",
              background: "var(--accent-soft)",
              animation: "lbPulseC 3.4s ease-out infinite",
            }}
          />

          {/* beams */}
          {beams.map((b) => (
            <div key={b.key}>
              <div
                style={{
                  position: "absolute",
                  left: b.x,
                  top: b.y,
                  width: b.w,
                  height: 2,
                  marginTop: -1,
                  background: b.grad,
                  transform: `rotate(${b.rot}deg)`,
                  transformOrigin: "0 50%",
                  borderRadius: 99,
                }}
              />
              <div
                style={{
                  position: "absolute",
                  left: b.x,
                  top: b.y,
                  width: b.w,
                  height: 4,
                  marginTop: -2,
                  background: b.dash,
                  transform: `rotate(${b.rot}deg)`,
                  transformOrigin: "0 50%",
                  animation: "lbDash .5s linear infinite",
                  borderRadius: 99,
                  opacity: 0.65,
                }}
              />
              <div
                style={{
                  position: "absolute",
                  left: b.chipX,
                  top: b.chipY,
                  width: 0,
                  height: 0,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  zIndex: 1,
                }}
              >
                <div
                  style={{
                    flex: "none",
                    whiteSpace: "nowrap",
                    background: b.chipBg,
                    color: "var(--accent-fg)",
                    fontFamily: "var(--mono)",
                    fontSize: 10,
                    fontWeight: 600,
                    padding: "2px 8px",
                    borderRadius: 99,
                    boxShadow: "0 2px 8px var(--ring-a)",
                  }}
                >
                  {b.pctStr}
                </div>
              </div>
            </div>
          ))}

          {/* center hub */}
          <div
            style={{
              position: "absolute",
              left: rcX,
              top: rcY - 29,
              width: 0,
              zIndex: 1,
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              gap: 7,
            }}
          >
            <div
              style={{
                width: 58,
                height: 58,
                borderRadius: "50%",
                background: "var(--panel)",
                border: "1px solid var(--border2)",
                boxShadow: "0 0 24px var(--ring-a)",
                display: "grid",
                placeItems: "center",
                fontSize: 12.5,
                fontWeight: 650,
                color: "var(--accent-ink)",
                flex: "none",
              }}
            >
              {t("devices.self")}
            </div>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 10.5,
                color: "var(--muted)",
                whiteSpace: "nowrap",
              }}
            >
              {deviceName}
            </div>
          </div>

          {/* device nodes */}
          {devices.map((d) => {
            const rp = nodePos[d.deviceId];
            if (!rp) return null;
            const name = nameById[d.deviceId] ?? d.name;
            const oldId = mismatchById[d.deviceId];
            const tr = runningByPeer[d.deviceId];
            const seen = firstSeen[d.deviceId];
            const isNew = !!seen && now - seen < 60_000;
            const target = dragDevice === d.deviceId;

            let note: string | null = null;
            let noteColor = "var(--muted)";
            if (oldId) {
              note = t("devices.fpChangedNote");
              noteColor = "var(--danger)";
            } else if (tr) {
              note =
                tr.direction === "send"
                  ? t("devices.sending")
                  : t("devices.receiving");
              noteColor =
                tr.direction === "receive"
                  ? "var(--success)"
                  : "var(--accent-ink)";
            } else if (isNew) {
              note = t("devices.justFound");
              noteColor = "var(--accent-ink)";
            }

            const ringColor = tr
              ? tr.direction === "receive"
                ? "var(--success)"
                : "var(--accent)"
              : "";
            const ringDeg = tr ? (tr.percent * 3.6).toFixed(1) : "0";

            const outer: CSSProperties = {
              position: "absolute",
              top: rp.y,
              zIndex: 2,
              transform: "translate(0,-50%)",
            };
            if (rp.mirror) outer.right = rW - rp.x - 23;
            else outer.left = rp.x - 23;

            return (
              <div key={d.deviceId} style={outer}>
                {/* biome-ignore lint/a11y/useSemanticElements: styled device row, not a native button — keeps the custom layout/markup while staying keyboard-operable */}
                <div
                  className="hover-row"
                  data-device-id={d.deviceId}
                  role="button"
                  tabIndex={0}
                  onClick={() => {
                    if (oldId) {
                      setFpAlert({ deviceId: oldId, step: "warn" });
                      return;
                    }
                    openSend(d.deviceId, recents);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      if (e.key === " ") e.preventDefault();
                      if (oldId) {
                        setFpAlert({ deviceId: oldId, step: "warn" });
                        return;
                      }
                      openSend(d.deviceId, recents);
                    }
                  }}
                  style={{
                    display: "flex",
                    flexDirection: rp.mirror ? "row-reverse" : "row",
                    alignItems: "center",
                    gap: 9,
                    padding: "8px 11px",
                    borderRadius: 12,
                    cursor: "pointer",
                    ...(target ? { background: "var(--accent-soft)" } : {}),
                    outline: target
                      ? "1.5px solid var(--accent)"
                      : "1.5px solid transparent",
                  }}
                >
                  {tr ? (
                    <div
                      style={{
                        width: 24,
                        height: 24,
                        borderRadius: "50%",
                        background: `conic-gradient(${ringColor} ${ringDeg}deg,var(--track) ${ringDeg}deg)`,
                        display: "grid",
                        placeItems: "center",
                        flex: "none",
                      }}
                    >
                      <div
                        style={{
                          width: 17,
                          height: 17,
                          borderRadius: "50%",
                          background: "var(--bg)",
                          display: "grid",
                          placeItems: "center",
                        }}
                      >
                        <div
                          style={{
                            width: 9,
                            height: 9,
                            borderRadius: "50%",
                            background: ringColor,
                            boxShadow: `0 0 9px ${ringColor}`,
                          }}
                        />
                      </div>
                    </div>
                  ) : (
                    <div
                      style={{
                        width: 24,
                        height: 24,
                        display: "grid",
                        placeItems: "center",
                        flex: "none",
                      }}
                    >
                      <div
                        style={{ position: "relative", width: 12, height: 12 }}
                      >
                        {isNew && (
                          <div
                            style={{
                              position: "absolute",
                              inset: -7,
                              borderRadius: "50%",
                              background: "var(--accent-soft)",
                              animation: "lbPulseDot 2.4s ease-out infinite",
                            }}
                          />
                        )}
                        <div
                          style={{
                            position: "absolute",
                            inset: 0,
                            borderRadius: "50%",
                            background: "var(--dot-live)",
                            boxShadow: "0 0 12px var(--glow)",
                          }}
                        />
                      </div>
                    </div>
                  )}
                  <div style={{ textAlign: rp.mirror ? "right" : "left" }}>
                    <div
                      style={{
                        fontSize: 12.5,
                        fontWeight: 600,
                        color: "var(--ink2)",
                        lineHeight: "18px",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {name}
                    </div>
                    <div
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 10.5,
                        color: "var(--muted)",
                        lineHeight: "14px",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {d.address}
                    </div>
                    {note && (
                      <div
                        style={{
                          fontSize: 10.5,
                          fontWeight: 600,
                          color: noteColor,
                          lineHeight: "14px",
                          whiteSpace: "nowrap",
                        }}
                      >
                        {note}
                      </div>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* ── empty-state checklist ──────────────────────────────────────── */}
      {devices.length === 0 && (
        <div
          style={{
            margin: "0 auto",
            width: 470,
            maxWidth: "calc(100% - 48px)",
            background: "var(--panel)",
            border: "1px solid var(--border)",
            borderRadius: 13,
            overflow: "hidden",
            animation: "lbFade .18s ease",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "baseline",
              gap: 8,
              padding: "12px 16px 4px",
            }}
          >
            <span
              style={{ fontSize: 12.5, fontWeight: 650, color: "var(--ink2)" }}
            >
              {t("devices.emptyTitle")}
            </span>
            <span style={{ fontSize: 10.5, color: "var(--muted)" }}>
              {t("devices.emptySub")}
            </span>
          </div>
          {(
            [
              [
                vis === "on" ? "✓" : "!",
                vis === "on" ? "var(--success)" : "var(--danger)",
                vis === "on"
                  ? t("devices.ck1On")
                  : vis === "ghost"
                    ? t("devices.ck1Ghost")
                    : t("devices.ck1Off"),
                "7px 16px",
                700,
              ],
              ["○", "var(--muted)", t("devices.ck2"), "7px 16px", 400],
              ["○", "var(--muted)", t("devices.ck3"), "7px 16px", 400],
              ["○", "var(--muted)", t("devices.ck4"), "7px 16px 11px", 400],
            ] as [string, string, string, string, number][]
          ).map(([mark, color, text, pad, weight], i) => (
            <div
              key={i}
              style={{
                display: "flex",
                alignItems: "baseline",
                gap: 9,
                padding: pad,
                fontSize: 11.5,
                color: "var(--muted2)",
              }}
            >
              <span style={{ color, fontWeight: weight, flex: "none" }}>
                {mark}
              </span>
              <span>{text}</span>
            </div>
          ))}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 9,
              padding: "11px 16px",
              borderTop: "1px solid var(--border)",
              background: "var(--sidebar)",
            }}
          >
            <input
              value={ipTry}
              onChange={(e) => setIpTry(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void tryDirect();
              }}
              placeholder="192.168.1.__"
              style={{
                flex: 1,
                minWidth: 0,
                height: 30,
                borderRadius: 8,
                border: "1px solid var(--border2)",
                background: "var(--panel)",
                color: "var(--ink)",
                padding: "0 11px",
                fontFamily: "var(--mono)",
                fontSize: 11,
                outline: "none",
              }}
            />
            <button
              type="button"
              onClick={() => void tryDirect()}
              onMouseEnter={(e) =>
                (e.currentTarget.style.filter = "brightness(.94)")
              }
              onMouseLeave={(e) => (e.currentTarget.style.filter = "none")}
              style={{
                height: 30,
                padding: "0 14px",
                borderRadius: 8,
                border: "none",
                background: "var(--accent)",
                color: "var(--accent-fg)",
                fontSize: 11.5,
                fontWeight: 600,
                fontFamily: "inherit",
                cursor: ipBusy ? "default" : "pointer",
                opacity: ipBusy ? 0.6 : 1,
                flex: "none",
              }}
            >
              {t("devices.ipDirect")}
            </button>
            {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native button — keeps the custom layout/markup while staying keyboard-operable */}
            <span
              role="button"
              tabIndex={0}
              onClick={() => setPair(true)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  if (e.key === " ") e.preventDefault();
                  setPair(true);
                }
              }}
              style={{
                fontSize: 11,
                color: "var(--accent-ink)",
                cursor: "pointer",
                flex: "none",
                whiteSpace: "nowrap",
              }}
            >
              {t("devices.pairInvite")}
            </span>
          </div>
        </div>
      )}

      {/* ── single-transfer strip ──────────────────────────────────────── */}
      {strip && (
        <div style={barBase}>
          <ExtChip
            ext={strip.ext ?? "FILE"}
            size={34}
            fontSize={9.5}
            radius={9}
          />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
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
                {(strip.fileCount ?? strip.files?.length ?? 1) > 1
                  ? t("transfers.filesMore", {
                      name: strip.name ?? "",
                      n: strip.fileCount ?? strip.files?.length ?? 1,
                    })
                  : (strip.name ?? "")}
              </span>
              <span
                style={{
                  fontSize: 11,
                  color: "var(--muted)",
                  whiteSpace: "nowrap",
                }}
              >
                {(strip.direction === "send" ? "→ " : "← ") +
                  (strip.peerName ?? "")}{" "}
                · {fmtBytes(strip.totalSize)}
              </span>
            </div>
            <div
              style={{
                height: 5,
                borderRadius: 99,
                background: "var(--track)",
                marginTop: 7,
                overflow: "hidden",
              }}
            >
              <div
                style={{
                  width: `${Math.min(strip.percent, 100)}%`,
                  height: "100%",
                  borderRadius: 99,
                  background:
                    strip.direction === "receive"
                      ? "var(--success)"
                      : "var(--accent)",
                }}
              />
            </div>
          </div>
          <div style={{ textAlign: "right", flex: "none" }}>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 12,
                color:
                  strip.direction === "receive"
                    ? "var(--success)"
                    : "var(--accent-ink)",
              }}
            >
              {mbps(strip.speedBps)}
            </div>
            <div style={{ fontSize: 10.5, color: "var(--muted)" }}>
              {etaClock(strip.totalSize, strip.percent, strip.speedBps)
                ? t("transfers.eta", {
                    t: etaClock(strip.totalSize, strip.percent, strip.speedBps),
                  })
                : ""}
            </div>
          </div>
          <div
            style={{
              display: "flex",
              gap: 4,
              flex: "none",
              alignItems: "center",
            }}
          >
            <button
              type="button"
              onClick={() => {
                // Cancel the single running transfer for real (M6.1); it moves to
                // history via transfer_error{code:"cancelled"}.
                void api.cancelTransfer(strip.sessionId);
                showToast(t("transfers.canceledToast"));
              }}
              onMouseEnter={(e) =>
                (e.currentTarget.style.color = "var(--danger)")
              }
              onMouseLeave={(e) =>
                (e.currentTarget.style.color = "var(--muted)")
              }
              style={{
                padding: "5px 11px",
                borderRadius: 7,
                fontSize: 12,
                color: "var(--muted)",
                border: "none",
                background: "none",
                cursor: "pointer",
                fontFamily: "inherit",
                whiteSpace: "nowrap",
              }}
            >
              {t("common.cancel")}
            </button>
            {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native button — keeps the custom layout/markup while staying keyboard-operable */}
            <span
              role="button"
              tabIndex={0}
              onClick={() => navigate("/transfers")}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  if (e.key === " ") e.preventDefault();
                  navigate("/transfers");
                }
              }}
              style={{
                fontSize: 11.5,
                color: "var(--accent-ink)",
                cursor: "pointer",
                marginLeft: 6,
                whiteSpace: "nowrap",
              }}
            >
              {t("common.all")}
            </span>
          </div>
        </div>
      )}

      {/* ── aggregate bar (≥2 transfers) ───────────────────────────────── */}
      {running.length >= 2 && (
        <div style={barBase}>
          <div style={{ display: "flex", flex: "none" }}>
            {aggExts.map((ext, i) => (
              <div
                key={ext}
                style={{
                  width: 30,
                  height: 30,
                  borderRadius: 9,
                  background: "var(--accent-soft)",
                  border: "2px solid var(--panel)",
                  display: "grid",
                  placeItems: "center",
                  fontFamily: "var(--mono)",
                  fontSize: 8,
                  fontWeight: 600,
                  color: "var(--accent-ink)",
                  marginLeft: i === 0 ? 0 : -9,
                }}
              >
                {ext}
              </div>
            ))}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
              <span
                style={{
                  fontSize: 12.5,
                  fontWeight: 600,
                  color: "var(--ink2)",
                  whiteSpace: "nowrap",
                }}
              >
                {t("devices.aggCount", { n: running.length })}
              </span>
              {nOut > 0 && (
                <span
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 10.5,
                    fontWeight: 600,
                    color: "var(--accent-ink)",
                    whiteSpace: "nowrap",
                  }}
                >
                  {t("devices.aggOut", { n: nOut })}
                </span>
              )}
              {nIn > 0 && (
                <span
                  style={{
                    fontFamily: "var(--mono)",
                    fontSize: 10.5,
                    fontWeight: 600,
                    color: "var(--success)",
                    whiteSpace: "nowrap",
                  }}
                >
                  {t("devices.aggIn", { n: nIn })}
                </span>
              )}
              <span
                style={{
                  fontSize: 11,
                  color: "var(--muted)",
                  whiteSpace: "nowrap",
                }}
              >
                {t("devices.aggTotal", { size: fmtBytes(aggSize) })}
              </span>
            </div>
            <div
              style={{
                height: 5,
                borderRadius: 99,
                background: "var(--track)",
                marginTop: 7,
                overflow: "hidden",
              }}
            >
              <div
                style={{
                  width: `${aggPct}%`,
                  height: "100%",
                  borderRadius: 99,
                  background: "var(--accent)",
                }}
              />
            </div>
          </div>
          <div style={{ textAlign: "right", flex: "none" }}>
            <div
              style={{
                fontFamily: "var(--mono)",
                fontSize: 12,
                color: "var(--accent-ink)",
              }}
            >
              {mbps(aggSpeedBps)}
            </div>
            <div style={{ fontSize: 10.5, color: "var(--muted)" }}>
              {aggEtaClock ? t("transfers.eta", { t: aggEtaClock }) : "—"}
            </div>
          </div>
          {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native button — keeps the custom layout/markup while staying keyboard-operable */}
          <span
            role="button"
            tabIndex={0}
            onClick={() => navigate("/transfers")}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                if (e.key === " ") e.preventDefault();
                navigate("/transfers");
              }
            }}
            style={{
              fontSize: 11.5,
              color: "var(--accent-ink)",
              cursor: "pointer",
              flex: "none",
              whiteSpace: "nowrap",
            }}
          >
            {t("common.all")}
          </span>
        </div>
      )}

      {/* ── bottom hint ────────────────────────────────────────────────── */}
      <div
        style={{
          textAlign: "center",
          fontSize: 11.5,
          color: "var(--muted)",
          padding: "13px 0 15px",
          letterSpacing: ".02em",
        }}
      >
        {devices.length ? t("devices.dragHint") : t("devices.emptyHint")}
      </div>
    </div>
  );
}
