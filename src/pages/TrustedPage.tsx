/** 受信任设备 · trust circle page.
 *  Three layouts by population: free chips (≤8), slot dots (9–20),
 *  list mode with circle overview (>20). Drag in/out of the ring toggles
 *  trust; the bottom-right remove zone forgets a remembered device. */
import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  showToast,
  trustList,
  useData,
  useOverlays,
  useTrust,
  type TrustDevice,
} from "../lib/store";
import { estChipW, shortName } from "../lib/format";
import { hashId, resolveChips, trustGeom, trustSlots } from "../lib/radar";
import { Toggle } from "../components/ui";

/* base canvas the free-mode positions are persisted on */
const BASE_CX = 320;
const BASE_CY = 215;

function inRemoveZone(
  p: { x: number; y: number },
  w: number,
  h: number,
): boolean {
  return p.x >= w - 130 && p.y >= h - 64;
}

/** Deterministic default spot on the 640×430 base canvas for a device with
 *  no stored position: trusted inside the ring, untrusted outside it. */
function defaultBasePos(
  id: string,
  trusted: boolean,
  R: number,
): { x: number; y: number } {
  const h = hashId(id);
  const a = ((h % 360) * Math.PI) / 180;
  const r = trusted
    ? 52 + ((h >> 8) % 64)
    : Math.min(190, R + 24 + ((h >> 8) % 40));
  return {
    x: Math.round(BASE_CX + r * Math.cos(a)),
    y: Math.round(BASE_CY + r * Math.sin(a)),
  };
}

/** Dashed drop target shown while dragging a node. */
function RemoveZone({ hot }: { hot: boolean }) {
  const { t } = useTranslation();
  return (
    <div
      style={{
        position: "absolute",
        right: 12,
        bottom: 10,
        width: 118,
        height: 54,
        border: `1.5px dashed ${hot ? "var(--danger)" : "var(--border2)"}`,
        borderRadius: 12,
        background: hot ? "var(--danger-soft)" : "transparent",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 1,
        zIndex: 5,
        pointerEvents: "none",
        animation: "lbFade .15s ease",
      }}
    >
      <span
        style={{
          fontSize: 11.5,
          fontWeight: 600,
          color: hot ? "var(--danger)" : "var(--muted2)",
        }}
      >
        {t("trusted.removeZone")}
      </span>
      <span
        style={{ fontSize: 9.5, color: hot ? "var(--danger)" : "var(--muted)" }}
      >
        {t("trusted.removeZoneSub")}
      </span>
    </div>
  );
}

type FreeNode = {
  id: string;
  x: number;
  y: number;
  mirror: boolean;
  inside: boolean;
  w: number;
  d: TrustDevice;
  sub: string;
  dragging: boolean;
};

export default function TrustedPage() {
  const { t } = useTranslation();
  const devices = useData((s) => s.devices);
  const records = useTrust((s) => s.records);
  const sel = useTrust((s) => s.sel);
  const setSel = useTrust((s) => s.setSel);
  const setTrust = useTrust((s) => s.setTrust);
  const toggleAuto = useTrust((s) => s.toggleAuto);
  const remove = useTrust((s) => s.remove);
  const restore = useTrust((s) => s.restore);
  const rename = useTrust((s) => s.rename);
  const setPos = useTrust((s) => s.setPos);
  const setFpAlert = useOverlays((s) => s.setFpAlert);

  const [box, setBox] = useState({ w: 640, h: 430 });
  const [drag, setDrag] = useState<{
    id: string;
    pos: { x: number; y: number };
  } | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [q, setQ] = useState("");
  /** session fallback for devices without a trust record (setPos needs one) */
  const [localPos, setLocalPos] = useState<
    Record<string, { x: number; y: number }>
  >({});
  const escRef = useRef(false);
  const trustWrapRef = useRef<HTMLDivElement | null>(null);
  const listWrapRef = useRef<HTMLDivElement | null>(null);
  const roRef = useRef<ResizeObserver | null>(null);

  /* measure the circle cell (works for both the circle and list cells) */
  const cellRef = useCallback((el: HTMLDivElement | null) => {
    roRef.current?.disconnect();
    roRef.current = null;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      if (!r.width || !r.height) return;
      const w = Math.round(Math.max(480, Math.min(720, r.width - 16)));
      const h = Math.round(Math.max(330, Math.min(470, r.height - 12)));
      setBox((b) =>
        Math.abs(w - b.w) > 1 || Math.abs(h - b.h) > 1 ? { w, h } : b,
      );
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    roRef.current = ro;
  }, []);

  const tl = trustList(devices, records);
  const mode: "free" | "slots" | "list" =
    tl.length > 20 ? "list" : tl.length > 8 ? "slots" : "free";

  /* ── geometry (from the prototype, scaled into the measured box) ────── */
  const tg = trustGeom(tl);
  const bw = Math.min(box.w, 640);
  const bh = Math.min(box.h, 430);
  const tcX = Math.round(bw / 2);
  const tcY = Math.round(bh / 2);
  const tk = Math.min(bw / 640, bh / 430, 1);
  const R2 = Math.round(tg.R * tk);
  const rT2 = Math.round(tg.rT * tk);
  const rU2 = Math.round(tg.rU * tk);
  const tnT = tl.filter((d) => d.trusted).length;
  const lrT = Math.min(120, 56 + tnT * 5);
  const lR = Math.min(160, lrT + 40);
  const lrU = Math.min(186, lR + 38);
  const lcW = Math.max(300, Math.min(430, box.w - 348));
  const lcX = Math.round(lcW / 2);
  const lcY = Math.round(bh / 2);
  const kl = Math.min(lcW / 400, bh / 430, 1);
  const lrT2 = Math.round(lrT * kl);
  const lR2 = Math.round(lR * kl);
  const lrU2 = Math.round(lrU * kl);
  const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;

  const basePosOf = (d: TrustDevice): { x: number; y: number } =>
    d.pos ??
    localPos[d.deviceId] ??
    defaultBasePos(d.deviceId, d.trusted, tg.R);

  const slotItems = tl.map((d) => ({ id: d.deviceId, trusted: d.trusted }));
  const slotMap =
    mode === "slots" ? trustSlots(slotItems, tcX, tcY, rT2, rU2, dpr) : {};
  const listMap =
    mode === "list" ? trustSlots(slotItems, lcX, lcY, lrT2, lrU2, dpr) : {};

  /* ── free-mode chips with collision resolution ──────────────────────── */
  let freeNodes: FreeNode[] = [];
  if (mode === "free") {
    freeNodes = tl.map((d) => {
      const p0 = basePosOf(d);
      const rp = {
        x: tcX + Math.round((p0.x - BASE_CX) * tk),
        y: tcY + Math.round((p0.y - BASE_CY) * tk),
      };
      const dragging = drag !== null && drag.id === d.deviceId;
      const p = drag !== null && drag.id === d.deviceId ? drag.pos : rp;
      const insideNow = Math.hypot(p.x - tcX, p.y - tcY) <= R2;
      const sub = d.fpChanged
        ? t("trusted.fpChangedShort")
        : d.trusted
          ? d.autoAccept
            ? t("trusted.trustedAuto")
            : t("trusted.trusted")
          : !d.online
            ? t("trusted.offline")
            : t("trusted.askEach");
      return {
        id: d.deviceId,
        x: p.x,
        y: p.y,
        mirror: p.x < tcX,
        inside: insideNow,
        w: estChipW(d.name, sub),
        d,
        sub,
        dragging,
      };
    });
    resolveChips(freeNodes, { tcX, tcY, R2, bw, bh, drag: drag?.id ?? null });
  }

  /* ── shared release helpers ─────────────────────────────────────────── */
  const removeWithUndo = (id: string, name: string) => {
    const rec = useTrust.getState().records[id];
    if (!rec) return;
    remove(id);
    /* 6 s window to undo, matching the prototype's removeDevice toast */
    showToast(
      t("trusted.removedToast", { name }),
      {
        label: t("common.undo"),
        fn: () => {
          restore(rec);
          showToast(t("trusted.restoredToast"));
        },
      },
      6000,
    );
  };

  /* ── drag: free + slots modes (circle box) ──────────────────────────── */
  const grabTrust = (e: ReactPointerEvent<HTMLDivElement>, id: string) => {
    if (e.button !== 0) return;
    const el = trustWrapRef.current;
    const d = tl.find((x) => x.deviceId === id);
    if (!el || !d) return;
    const rect = el.getBoundingClientRect();
    const base = basePosOf(d);
    const pos =
      mode === "free"
        ? {
            x: tcX + Math.round((base.x - BASE_CX) * tk),
            y: tcY + Math.round((base.y - BASE_CY) * tk),
          }
        : (slotMap[id] ?? { x: tcX, y: tcY });
    const offX = e.clientX - rect.left - pos.x;
    const offY = e.clientY - rect.top - pos.y;
    let moved = false;
    let last = pos;
    e.preventDefault();
    const mv = (ev: PointerEvent) => {
      const nx = Math.round(
        Math.min(bw - 22, Math.max(22, ev.clientX - rect.left - offX)),
      );
      const ny = Math.round(
        Math.min(bh - 24, Math.max(24, ev.clientY - rect.top - offY)),
      );
      if (!moved && Math.hypot(nx - pos.x, ny - pos.y) > 3) moved = true;
      if (!moved) return;
      last = { x: nx, y: ny };
      setDrag({ id, pos: last });
    };
    const up = () => {
      window.removeEventListener("pointermove", mv);
      window.removeEventListener("pointerup", up);
      setDrag(null);
      if (!moved) {
        setSel(id);
        return;
      }
      /* only remembered devices can be forgotten — otherwise treat the
         corner like any other drop spot */
      if (inRemoveZone(last, bw, bh) && useTrust.getState().records[id]) {
        removeWithUndo(id, d.name);
        return;
      }
      const inside = Math.hypot(last.x - tcX, last.y - tcY) <= R2;
      const was = d.trusted;
      if (inside !== was) setTrust({ deviceId: id, name: d.name }, inside);
      if (mode === "free") {
        /* nudge clear of neighbours, snap out of the ring band, clamp */
        let fx = last.x;
        let fy = last.y;
        for (const o of tl) {
          if (o.deviceId === id) continue;
          const ob = basePosOf(o);
          const ox = tcX + Math.round((ob.x - BASE_CX) * tk);
          const oy = tcY + Math.round((ob.y - BASE_CY) * tk);
          const dist = Math.hypot(fx - ox, fy - oy);
          if (dist < 54 && dist > 0.1) {
            fx = ox + ((fx - ox) / dist) * 54;
            fy = oy + ((fy - oy) / dist) * 54;
          }
        }
        const rr = Math.hypot(fx - tcX, fy - tcY) || 1;
        if (inside && rr > R2 - 16) {
          fx = tcX + ((fx - tcX) / rr) * (R2 - 16);
          fy = tcY + ((fy - tcY) / rr) * (R2 - 16);
        }
        if (!inside && rr < R2 + 16) {
          fx = tcX + ((fx - tcX) / rr) * (R2 + 16);
          fy = tcY + ((fy - tcY) / rr) * (R2 + 16);
        }
        fx = Math.min(bw - 22, Math.max(22, fx));
        fy = Math.min(bh - 24, Math.max(24, fy));
        const bx = Math.round(
          Math.min(618, Math.max(22, BASE_CX + (fx - tcX) / tk)),
        );
        const by = Math.round(
          Math.min(406, Math.max(24, BASE_CY + (fy - tcY) / tk)),
        );
        setLocalPos((m) => ({ ...m, [id]: { x: bx, y: by } }));
        setPos(id, { x: bx, y: by });
      }
      setSel(id);
      if (inside && !was) showToast(t("trusted.addedToast", { name: d.name }));
      if (!inside && was)
        showToast(t("trusted.removedFromCircleToast", { name: d.name }));
    };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", up);
  };

  /* ── drag: list-mode overview dots ──────────────────────────────────── */
  const grabTrustList = (e: ReactPointerEvent<HTMLDivElement>, id: string) => {
    if (e.button !== 0) return;
    const el = listWrapRef.current;
    const d = tl.find((x) => x.deviceId === id);
    if (!el || !d) return;
    const rect = el.getBoundingClientRect();
    const sx = e.clientX;
    const sy = e.clientY;
    let moved = false;
    let last = { x: lcX, y: lcY };
    e.preventDefault();
    const mv = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - sx, ev.clientY - sy) > 3)
        moved = true;
      if (!moved) return;
      last = {
        x: Math.round(
          Math.min(rect.width - 10, Math.max(10, ev.clientX - rect.left)),
        ),
        y: Math.round(
          Math.min(rect.height - 20, Math.max(20, ev.clientY - rect.top)),
        ),
      };
      setDrag({ id, pos: last });
    };
    const up = () => {
      window.removeEventListener("pointermove", mv);
      window.removeEventListener("pointerup", up);
      setDrag(null);
      if (!moved) {
        setSel(id);
        return;
      }
      if (
        inRemoveZone(last, rect.width, rect.height) &&
        useTrust.getState().records[id]
      ) {
        removeWithUndo(id, d.name);
        return;
      }
      const inside = Math.hypot(last.x - lcX, last.y - lcY) <= lR2;
      setSel(id);
      if (inside !== d.trusted)
        setTrust({ deviceId: id, name: d.name }, inside);
    };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", up);
  };

  /* ── list rows (search + trusted first) ─────────────────────────────── */
  const qn = q.trim().toLowerCase();
  const rows =
    mode === "list"
      ? tl
          .filter((d) => !qn || d.name.toLowerCase().includes(qn))
          .sort((a, b) => (b.trusted ? 1 : 0) - (a.trusted ? 1 : 0))
      : [];

  /* ── selection bar model ────────────────────────────────────────────── */
  const selD = tl.find((d) => d.deviceId === sel) ?? tl[0] ?? null;
  const selDotBg = selD
    ? selD.fpChanged
      ? "var(--danger)"
      : selD.trusted
        ? "var(--dot-live)"
        : "var(--muted)"
    : "var(--muted)";
  const selDotShadow = selD?.trusted ? "0 0 9px var(--glow)" : "none";

  const commitRename = (v: string) => {
    const name = v.trim();
    if (!escRef.current && name && editing) rename(editing, name);
    escRef.current = false;
    setEditing(null);
  };

  /* chip pointerdown calls preventDefault(), which suppresses the rename
     input's blur — if the selection then moves elsewhere the input unmounts
     without ever firing onBlur, leaving `editing` stale. Tear it down as
     soon as it no longer matches the device shown in the bar. */
  const selId = selD?.deviceId ?? null;
  useEffect(() => {
    if (editing && editing !== selId) {
      escRef.current = false;
      setEditing(null);
    }
  }, [editing, selId]);

  /* the remove zone only applies to remembered devices (those with a
     trust record) — never advertise a drop target that would no-op */
  const dragRemovable = drag !== null && Boolean(records[drag.id]);
  const hotC = drag && dragRemovable ? inRemoveZone(drag.pos, bw, bh) : false;
  const hotL = drag && dragRemovable ? inRemoveZone(drag.pos, lcW, bh) : false;
  const hint =
    mode === "free"
      ? t("trusted.hintFree")
      : mode === "slots"
        ? t("trusted.hintSlots")
        : t("trusted.hintList");

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
        overflow: "hidden",
        animation: "lbFade .18s ease",
      }}
    >
      {mode !== "list" ? (
        /* ── circle cell (free + slots) ───────────────────────────────── */
        <div
          ref={cellRef}
          style={{
            flex: 1,
            display: "grid",
            placeItems: "center",
            minHeight: 0,
            padding: "8px 24px 0",
            overflow: "hidden",
          }}
        >
          <div
            ref={trustWrapRef}
            style={{
              position: "relative",
              width: bw,
              height: bh,
              flex: "none",
            }}
          >
            <div
              style={{
                position: "absolute",
                left: tcX,
                top: tcY,
                width: R2 * 2,
                height: R2 * 2,
                margin: `-${R2}px 0 0 -${R2}px`,
                borderRadius: "50%",
                background: "var(--accent-soft)",
                opacity: 0.5,
              }}
            />
            <div
              style={{
                position: "absolute",
                left: tcX,
                top: tcY,
                width: R2 * 2,
                height: R2 * 2,
                margin: `-${R2}px 0 0 -${R2}px`,
                border: "1px dashed var(--accent)",
                borderRadius: "50%",
                opacity: 0.5,
              }}
            />
            <div
              style={{
                position: "absolute",
                left: tcX,
                top: tcY - R2,
                width: 64,
                margin: "-7px 0 0 -32px",
                textAlign: "center",
                lineHeight: "14px",
                fontFamily: "var(--mono)",
                fontSize: 10.5,
                color: "var(--accent-ink)",
                background: "var(--bg)",
                zIndex: 1,
              }}
            >
              {t("trusted.circle")}
            </div>
            <div
              style={{
                position: "absolute",
                left: tcX,
                top: tcY,
                margin: "-23px 0 0 -23px",
                width: 46,
                height: 46,
                borderRadius: "50%",
                background: "var(--panel)",
                border: "1px solid var(--border2)",
                boxShadow: "0 0 18px var(--ring-a)",
                display: "grid",
                placeItems: "center",
                fontSize: 11,
                fontWeight: 650,
                color: "var(--accent-ink)",
              }}
            >
              {t("trusted.self")}
            </div>
            {drag && dragRemovable && <RemoveZone hot={hotC} />}

            {mode === "free" &&
              freeNodes.map((n) => {
                const selMe = sel === n.id;
                const online = n.d.online;
                const wrap: CSSProperties = {
                  position: "absolute",
                  top: Math.round(n.y),
                  zIndex: n.dragging ? 6 : 3,
                  transform: "translate(0,-50%)",
                  opacity: online ? 1 : 0.55,
                  touchAction: "none",
                  userSelect: "none",
                  cursor: n.dragging ? "grabbing" : "grab",
                };
                if (n.mirror) wrap.right = Math.round(bw - n.x - 16);
                else wrap.left = Math.round(n.x - 16);
                return (
                  <div
                    key={n.id}
                    onPointerDown={(e) => grabTrust(e, n.id)}
                    style={wrap}
                  >
                    {/* biome-ignore lint/a11y/noStaticElementInteractions: decorative hover-only styling — the node's drag/select action lives on the parent's onPointerDown, not here */}
                    <div
                      style={{
                        display: "flex",
                        flexDirection: n.mirror ? "row-reverse" : "row",
                        alignItems: "center",
                        gap: 8,
                        padding: "6px 10px",
                        borderRadius: 11,
                        background: selMe
                          ? "var(--accent-soft)"
                          : "transparent",
                        outline: n.dragging
                          ? "1.5px solid var(--accent)"
                          : "1.5px solid transparent",
                      }}
                      onMouseEnter={(e) => {
                        if (!selMe)
                          e.currentTarget.style.background = "var(--hover)";
                      }}
                      onMouseLeave={(e) => {
                        e.currentTarget.style.background = selMe
                          ? "var(--accent-soft)"
                          : "transparent";
                      }}
                    >
                      <div
                        style={{
                          width: 12,
                          height: 12,
                          borderRadius: "50%",
                          background: !online
                            ? "transparent"
                            : n.d.trusted
                              ? "var(--dot-live)"
                              : "var(--muted)",
                          border: !online ? "1.5px solid var(--muted)" : "none",
                          boxShadow: n.d.trusted
                            ? "0 0 10px var(--glow)"
                            : "none",
                          flex: "none",
                        }}
                      />
                      <div style={{ textAlign: n.mirror ? "right" : "left" }}>
                        <div
                          style={{
                            fontSize: 12,
                            fontWeight: 600,
                            color: "var(--ink2)",
                            lineHeight: "16px",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {n.d.name}
                        </div>
                        <div
                          style={{
                            fontFamily: "var(--mono)",
                            fontSize: 10,
                            color: n.d.fpChanged
                              ? "var(--danger)"
                              : n.d.trusted
                                ? "var(--accent-ink)"
                                : "var(--muted)",
                            lineHeight: "14px",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {n.sub}
                        </div>
                      </div>
                    </div>
                  </div>
                );
              })}

            {mode === "slots" &&
              tl.map((d) => {
                const dragging = drag !== null && drag.id === d.deviceId;
                const p =
                  drag !== null && drag.id === d.deviceId
                    ? drag.pos
                    : (slotMap[d.deviceId] ?? { x: tcX, y: tcY });
                const selMe = sel === d.deviceId;
                const glow = d.trusted ? "0 0 10px var(--glow)" : "none";
                return (
                  <div
                    key={d.deviceId}
                    onPointerDown={(e) => grabTrust(e, d.deviceId)}
                    title={d.name}
                    style={{
                      position: "absolute",
                      left: p.x,
                      top: p.y,
                      zIndex: dragging ? 6 : 3,
                      width: 84,
                      marginLeft: -42,
                      marginTop: -8,
                      opacity: d.online ? 1 : 0.55,
                      touchAction: "none",
                      userSelect: "none",
                      cursor: dragging ? "grabbing" : "grab",
                      textAlign: "center",
                      padding: "2px 0",
                    }}
                  >
                    <div
                      style={{
                        width: 12,
                        height: 12,
                        borderRadius: "50%",
                        background: !d.online
                          ? "transparent"
                          : d.trusted
                            ? "var(--dot-live)"
                            : "var(--muted)",
                        border: !d.online ? "1.5px solid var(--muted)" : "none",
                        boxShadow: selMe
                          ? d.trusted
                            ? "0 0 10px var(--glow),0 0 0 3px var(--accent-soft)"
                            : "0 0 0 3px var(--accent-soft)"
                          : glow,
                        margin: "0 auto",
                      }}
                    />
                    <div
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 10,
                        lineHeight: "14px",
                        color: d.trusted
                          ? "var(--accent-ink)"
                          : "var(--muted2)",
                        marginTop: 4,
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {shortName(d.name)}
                    </div>
                  </div>
                );
              })}
          </div>
        </div>
      ) : (
        /* ── list cell: circle overview + searchable list ─────────────── */
        <div
          ref={cellRef}
          style={{
            flex: 1,
            display: "grid",
            placeItems: "center",
            minHeight: 0,
            padding: "12px 24px 0",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              display: "flex",
              gap: 18,
              flex: "none",
              alignItems: "stretch",
            }}
          >
            <div
              ref={listWrapRef}
              style={{
                position: "relative",
                width: lcW,
                height: bh,
                flex: "none",
              }}
            >
              <div
                style={{
                  position: "absolute",
                  left: lcX,
                  top: lcY,
                  width: lR2 * 2,
                  height: lR2 * 2,
                  margin: `-${lR2}px 0 0 -${lR2}px`,
                  borderRadius: "50%",
                  background: "var(--accent-soft)",
                  opacity: 0.5,
                }}
              />
              <div
                style={{
                  position: "absolute",
                  left: lcX,
                  top: lcY,
                  width: lR2 * 2,
                  height: lR2 * 2,
                  margin: `-${lR2}px 0 0 -${lR2}px`,
                  border: "1px dashed var(--accent)",
                  borderRadius: "50%",
                  opacity: 0.5,
                }}
              />
              <div
                style={{
                  position: "absolute",
                  left: lcX,
                  top: lcY,
                  margin: "-17px 0 0 -17px",
                  width: 34,
                  height: 34,
                  borderRadius: "50%",
                  background: "var(--panel)",
                  border: "1px solid var(--border2)",
                  display: "grid",
                  placeItems: "center",
                  fontSize: 10,
                  fontWeight: 650,
                  color: "var(--accent-ink)",
                  zIndex: 1,
                }}
              >
                {t("trusted.self")}
              </div>
              {tl.map((d) => {
                const selMe = sel === d.deviceId;
                const dragging = drag !== null && drag.id === d.deviceId;
                const p =
                  drag !== null && drag.id === d.deviceId
                    ? drag.pos
                    : (listMap[d.deviceId] ?? { x: lcX, y: lcY });
                const size = selMe ? 14 : 10;
                return (
                  <div
                    key={d.deviceId}
                    onPointerDown={(e) => grabTrustList(e, d.deviceId)}
                    title={d.name}
                    style={{
                      position: "absolute",
                      left: p.x,
                      top: p.y,
                      zIndex: dragging ? 6 : 2,
                      width: 72,
                      marginLeft: -36,
                      marginTop: -8,
                      opacity: d.online ? 1 : 0.55,
                      touchAction: "none",
                      userSelect: "none",
                      cursor: dragging ? "grabbing" : "grab",
                      textAlign: "center",
                      padding: "2px 0",
                    }}
                  >
                    <div
                      style={{
                        width: size,
                        height: size,
                        borderRadius: "50%",
                        background: d.trusted
                          ? "var(--dot-live)"
                          : "var(--muted)",
                        boxShadow: selMe
                          ? "0 0 0 3px var(--accent-soft)" +
                            (d.trusted ? ",0 0 10px var(--glow)" : "")
                          : d.trusted
                            ? "0 0 8px var(--glow)"
                            : "none",
                        margin: "0 auto",
                      }}
                    />
                    <div
                      style={{
                        fontFamily: "var(--mono)",
                        fontSize: 9.5,
                        lineHeight: "13px",
                        color: d.trusted
                          ? "var(--accent-ink)"
                          : "var(--muted2)",
                        marginTop: 3,
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {shortName(d.name)}
                    </div>
                  </div>
                );
              })}
              <div
                style={{
                  position: "absolute",
                  left: 8,
                  bottom: 6,
                  fontFamily: "var(--mono)",
                  fontSize: 10,
                  color: "var(--muted)",
                }}
              >
                {t("trusted.countLine", { t: tnT, n: tl.length })}
              </div>
              {drag && dragRemovable && <RemoveZone hot={hotL} />}
            </div>

            <div
              style={{
                width: 330,
                height: bh,
                flex: "none",
                display: "flex",
                flexDirection: "column",
                gap: 8,
                minWidth: 0,
              }}
            >
              <input
                value={q}
                onChange={(e) => setQ(e.target.value)}
                placeholder={t("trusted.searchPlaceholder")}
                style={{
                  height: 32,
                  padding: "0 11px",
                  borderRadius: 8,
                  border: "1px solid var(--border2)",
                  background: "var(--panel)",
                  color: "var(--ink)",
                  fontSize: 12,
                  fontFamily: "inherit",
                  outline: "none",
                }}
              />
              <div className="card scroll-y" style={{ flex: 1 }}>
                {rows.length === 0 ? (
                  <div
                    style={{
                      padding: 22,
                      textAlign: "center",
                      fontSize: 11.5,
                      color: "var(--muted)",
                    }}
                  >
                    {t("trusted.noMatch")}
                  </div>
                ) : (
                  rows.map((d) => (
                    // biome-ignore lint/a11y/useSemanticElements: styled device row, not a native button — keeps the custom layout/markup while staying keyboard-operable via role/tabIndex/onKeyDown
                    <div
                      key={d.deviceId}
                      role="button"
                      tabIndex={0}
                      onClick={() => setSel(d.deviceId)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          if (e.key === " ") e.preventDefault();
                          setSel(d.deviceId);
                        }
                      }}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 10,
                        padding: "9px 13px",
                        borderBottom: "1px solid var(--border)",
                        cursor: "pointer",
                        background:
                          sel === d.deviceId
                            ? "var(--accent-soft)"
                            : "transparent",
                      }}
                    >
                      <span
                        style={{
                          width: 8,
                          height: 8,
                          borderRadius: "50%",
                          background: d.trusted
                            ? "var(--dot-live)"
                            : "var(--muted)",
                          boxShadow: d.trusted ? "0 0 8px var(--glow)" : "none",
                          flex: "none",
                        }}
                      />
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          style={{
                            fontSize: 12,
                            fontWeight: 600,
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
                            fontSize: 9.5,
                            color: "var(--muted)",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {d.address ?? d.fp}
                        </div>
                      </div>
                      <Toggle
                        on={d.trusted}
                        size="sm"
                        stop
                        onClick={() =>
                          setTrust(
                            { deviceId: d.deviceId, name: d.name },
                            !d.trusted,
                          )
                        }
                      />
                    </div>
                  ))
                )}
              </div>
            </div>
          </div>
        </div>
      )}

      {/* ── selected-device bar ─────────────────────────────────────────── */}
      <div
        style={{
          display: "flex",
          justifyContent: "center",
          padding: "12px 24px 0",
        }}
      >
        <div
          style={{
            width: 640,
            maxWidth: "100%",
            background: "var(--panel)",
            border: "1px solid var(--border)",
            borderRadius: 12,
            padding: "11px 16px",
            display: "flex",
            alignItems: "center",
            gap: 11,
          }}
        >
          <span
            style={{
              width: 9,
              height: 9,
              borderRadius: "50%",
              background: selDotBg,
              boxShadow: selDotShadow,
              flex: "none",
            }}
          />
          {selD ? (
            editing === selD.deviceId ? (
              <input
                defaultValue={selD.name}
                // biome-ignore lint/a11y/noAutofocus: the rename/edit field autofocuses when opened (deliberate UX)
                autoFocus
                onBlur={(e) => commitRename(e.currentTarget.value)}
                onKeyDown={(e) => {
                  /* Enter/Escape aimed at the IME candidate window must not
                     commit or cancel the rename (keyCode 229 covers WebKit,
                     where the confirm keydown lands after compositionend) */
                  if (e.nativeEvent.isComposing || e.keyCode === 229) return;
                  if (e.key === "Enter") e.currentTarget.blur();
                  if (e.key === "Escape") {
                    escRef.current = true;
                    e.currentTarget.blur();
                  }
                }}
                style={{
                  height: 26,
                  width: 160,
                  padding: "0 8px",
                  borderRadius: 7,
                  border: "1px solid var(--accent)",
                  background: "var(--bg)",
                  color: "var(--ink)",
                  fontSize: 12,
                  fontWeight: 600,
                  fontFamily: "inherit",
                  outline: "none",
                }}
              />
            ) : (
              <>
                <span
                  style={{
                    fontSize: 12.5,
                    fontWeight: 600,
                    color: "var(--ink2)",
                    whiteSpace: "nowrap",
                  }}
                >
                  {selD.name}
                </span>
                {/* biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native button — keeps the custom layout/markup while staying keyboard-operable via role/tabIndex/onKeyDown */}
                <span
                  role="button"
                  tabIndex={0}
                  onClick={() => setEditing(selD.deviceId)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      if (e.key === " ") e.preventDefault();
                      setEditing(selD.deviceId);
                    }
                  }}
                  style={{
                    fontSize: 11,
                    color: "var(--accent-ink)",
                    cursor: "pointer",
                    whiteSpace: "nowrap",
                  }}
                >
                  {t("common.rename")}
                </span>
              </>
            )
          ) : (
            <span
              style={{
                fontSize: 12.5,
                fontWeight: 600,
                color: "var(--ink2)",
                whiteSpace: "nowrap",
              }}
            >
              {t("trusted.noDevices")}
            </span>
          )}
          <span
            style={{
              fontFamily: "var(--mono)",
              fontSize: 10,
              color: "var(--muted2)",
              background: "var(--sidebar)",
              borderRadius: 6,
              padding: "3px 8px",
              whiteSpace: "nowrap",
            }}
          >
            {selD?.fp ?? "—"}
          </span>
          <div style={{ flex: 1 }} />
          {selD?.trusted && (
            <>
              <span style={{ fontSize: 11.5, color: "var(--muted2)" }}>
                {t("trusted.autoAccept")}
              </span>
              <Toggle
                on={selD.autoAccept}
                onClick={() => toggleAuto(selD.deviceId)}
              />
            </>
          )}
          {selD && !selD.trusted && !selD.fpChanged && (
            <span style={{ fontSize: 11.5, color: "var(--muted)" }}>
              {t("trusted.untrustedNote")}
            </span>
          )}
          {selD?.fpChanged && (
            // biome-ignore lint/a11y/useSemanticElements: styled inline action, not a native button — keeps the custom layout/markup while staying keyboard-operable via role/tabIndex/onKeyDown
            <span
              role="button"
              tabIndex={0}
              onClick={() =>
                setFpAlert({ deviceId: selD.deviceId, step: "warn" })
              }
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  if (e.key === " ") e.preventDefault();
                  setFpAlert({ deviceId: selD.deviceId, step: "warn" });
                }
              }}
              style={{
                fontSize: 11.5,
                fontWeight: 600,
                color: "var(--danger)",
                cursor: "pointer",
              }}
            >
              {t("trusted.fpChangedAction")}
            </span>
          )}
        </div>
      </div>

      <div
        style={{
          textAlign: "center",
          fontSize: 11.5,
          color: "var(--muted)",
          padding: "12px 0 14px",
          letterSpacing: ".02em",
        }}
      >
        {hint}
      </div>
    </div>
  );
}
