import {
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import i18n from "../i18n";
import * as api from "../bridge/api";
import type { PairJoinedEvent } from "../bridge/api";
import {
  type PairPrefill,
  showToast,
  useData,
  useOverlays,
  useTrust,
} from "../lib/store";
import { copyText, errText } from "../lib/sendops";
import { fmtCountdown, fmtSas } from "../lib/format";
import {
  type PairAction,
  type PairState,
  fmtLinkPreview,
  fmtPairCode,
  initialPairState,
  joinErrorOf,
  pairReducer,
  validateAddr,
  validateJoin,
} from "../lib/pairing";
import { useEscape } from "../lib/useEscape";
import { MODAL_W, ModalHead, Segmented } from "./ui";

/** Every async result is tagged with a unique, increasing epoch (see
 *  lib/pairing.ts). Module-wide so two mounts can never reuse one. */
let epochSeq = 0;
const nextEpoch = () => ++epochSeq;

/** 配对设备 — two views and one ending.
 *
 *  「让对方加入我」(host) shows this device's address and a 6-digit pairing code:
 *  the two things the other side has to type. 「加入对方」(join) is where you
 *  type them (or paste the pair link that packs both). Either way you land on
 *  the SAS compare step, where both screens show the same number.
 *
 *  Redeeming a code and trusting a device are two different decisions, and only
 *  the second one needs a human — the SAS is the sole defence against a
 *  machine-in-the-middle, and a code nobody read is not a check. So neither
 *  `join_by_code` nor the host's pairing handler grants any trust; it is
 *  recorded HERE, through the same `useTrust.setTrust` path the trust circle
 *  uses (which is what also turns on auto-accept), and only once the user says
 *  the two screens agree. Dismissing the compare step trusts nothing.
 *
 *  The state machine — and every rule about the two roles racing each other —
 *  lives in lib/pairing.ts. This component runs the side effects. */
export default function PairModal() {
  const { t } = useTranslation();
  const pairOpen = useOverlays((s) => s.pairOpen);
  const prefill = useOverlays((s) => s.pairPrefill);
  const setPair = useOverlays((s) => s.setPair);

  // The reducer's state, mirrored in a ref that is updated SYNCHRONOUSLY on
  // every dispatch: async continuations and double-click guards read `live`,
  // never a render that may not have happened yet.
  const [st, setSt] = useState<PairState>(() => initialPairState());
  const live = useRef(st);
  const send = useCallback((a: PairAction) => {
    live.current = pairReducer(live.current, a);
    setSt(live.current);
  }, []);
  // The prefill object this open already applied (they are applied once each,
  // by identity — see OverlayState.pairPrefill).
  const seenPrefill = useRef<PairPrefill | null>(null);
  // True only when the pointer went down on the scrim itself; a drag that
  // starts inside the modal and ends on the scrim dispatches click on the
  // scrim, which must not dismiss the modal.
  const scrimDown = useRef(false);

  /* ── open / close ──────────────────────────────────────────────────────
     Every way the overlay closes runs this cleanup, so it is the one place
     that invalidates the code and drops whatever is still in flight. A layout
     effect, so the first painted frame is already the initialized form. */
  useLayoutEffect(() => {
    if (!pairOpen) return;
    const p = useOverlays.getState().pairPrefill;
    seenPrefill.current = p;
    send({ type: "open", epoch: nextEpoch(), prefill: p });
    const off = api.onEvent<PairJoinedEvent>("pair_joined", (e) =>
      send({
        type: "joined",
        peer: { deviceId: e.deviceId, name: e.name, sas: e.sas },
      }),
    );
    return () => {
      off();
      void api.cancelPairing().catch(() => {});
      send({ type: "close", epoch: nextEpoch() });
    };
  }, [pairOpen, send]);

  /* ── a prefill that arrives while the form is already open ─────────────
     (a second deep link). Taken unless a join or a SAS compare is underway —
     swapping the fields under either would be worse than saying "not now". */
  useEffect(() => {
    if (!pairOpen || !prefill || prefill === seenPrefill.current) return;
    seenPrefill.current = prefill;
    const s = live.current;
    if (s.busy || s.compare) {
      showToast(t("pair.busyPrefill"));
      return;
    }
    send({ type: "prefill", prefill });
  }, [pairOpen, prefill, send, t]);

  /* ── host: mint a code ───────────────────────────────────────────────── */
  const mint = useCallback(() => {
    const ep = nextEpoch();
    send({ type: "mintStart", epoch: ep });
    if (live.current.epoch !== ep) return; // refused (a join / compare is on)
    api.startPairing().then(
      (invite) => send({ type: "minted", epoch: ep, invite }),
      (e) => {
        const mine = live.current.epoch === ep && live.current.minting;
        send({ type: "mintFailed", epoch: ep });
        // Don't leave a blank code and a "waiting…" behind a swallowed error.
        if (mine) showToast(i18n.t("pair.codeFailed", { err: errText(e) }));
      },
    );
  }, [send]);

  // The host view always shows a live code: mint on arrival (first open, or
  // switching back after a join cancelled the old one). Not after a failure —
  // that waits for the user's 换一个, never a retry loop.
  useEffect(() => {
    if (!st.open || st.role !== "host" || st.compare || st.busy) return;
    if (st.invite || st.minting || st.mintFailed) return;
    // A second run for the same render (StrictMode) must not mint twice.
    if (live.current.minting) return;
    mint();
  }, [
    st.open,
    st.role,
    st.invite,
    st.minting,
    st.mintFailed,
    st.busy,
    st.compare,
    mint,
  ]);

  const regen = () => {
    const replacing = !!live.current.invite;
    mint();
    // Only a REPLACED code has anything to warn about.
    if (replacing) showToast(t("pair.regenToast"));
  };

  /* ── countdown ─────────────────────────────────────────────────────── */
  const [now, setNow] = useState(() => Date.now());
  // 0 = no expiry known (never from the real backend): no countdown, and
  // never shown as expired on a guess.
  const expiresAt = st.invite?.expiresAt ?? 0;
  const remaining = expiresAt - now;
  const expired = !!st.invite && expiresAt > 0 && remaining <= 0;
  useEffect(() => {
    if (!expiresAt) return;
    setNow(Date.now());
    if (expiresAt <= Date.now()) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [expiresAt]);

  /* ── closing, and the compare verdicts ───────────────────────────────── */
  // Walking away from the compare step is a "no" — nothing was trusted. Say so,
  // or the user leaves believing they paired.
  const close = () => {
    if (live.current.compare) showToast(t("pair.unconfirmedToast"));
    setPair(false);
  };
  useEscape(pairOpen, close);

  /** The two screens agree → record the trust, through the SAME path the trust
   *  circle uses (so a paired device is trusted AND auto-accepting, exactly like
   *  one dragged into the circle — there is no second, divergent notion of
   *  trust for pairing to drift away from). */
  const acceptPeer = () => {
    const c = live.current.compare;
    if (!c) return;
    useTrust.getState().setTrust({ deviceId: c.deviceId, name: c.name }, true);
    showToast(t("pair.trustedToast", { name: c.name }));
    setPair(false);
  };

  const rejectPeer = () => {
    const c = live.current.compare;
    // The screens disagree, so whoever answered may not be the device you meant.
    // Drop the address this pairing filed for it: the backend keeps that table
    // on disk, so it would otherwise stay in the device list for good, under a
    // name that device picked for itself. A device you already trust keeps its
    // entry: its id is its key, so the handshake did reach that very device.
    if (c && !useTrust.getState().records[c.deviceId]?.trusted)
      void api.forgetDevice(c.deviceId).catch(() => {});
    showToast(t("pair.mismatchToast"), undefined, 8000);
    setPair(false);
  };

  /* ── join ──────────────────────────────────────────────────────────── */
  const refreshDevices = () =>
    // The backend just recorded the peer in its manual table — discovery may
    // never announce it (a different subnet / discovery off: the exact case
    // pairing by address exists for), so it surfaces through the merged
    // list_discovered_devices, not a discovery event. Re-query so it shows in
    // the radar / SendModal right away. It shows up UNTRUSTED — being reachable
    // and being trusted are separate, and trust is still one SAS compare away.
    api
      .listDiscoveredDevices()
      .then((list) => useData.getState().setDevices(list))
      .catch(() => {});

  const doJoin = async () => {
    const s = live.current;
    if (s.busy || s.compare) return;
    const v = validateJoin(s.addr, s.code);
    if (!v.ok) {
      send({ type: "error", error: v.error });
      return;
    }
    const ep = nextEpoch();
    send({ type: "joinStart", epoch: ep });
    if (live.current.epoch !== ep) return;
    // A device that is joining someone must not also be redeemable as a host:
    // cancel OUR code before dialing theirs. Two people pressing Join at each
    // other then get two clean rejections, never two sessions with two SASes.
    await api.cancelPairing().catch(() => {});
    if (live.current.epoch !== ep) return; // closed meanwhile: don't dial
    try {
      const r = await api.joinByCode(v.addr, v.code, s.expectedId ?? undefined);
      send({ type: "joinDone", epoch: ep, peer: r });
      void refreshDevices();
    } catch (e) {
      send({ type: "joinFailed", epoch: ep, error: joinErrorOf(e, v.addr) });
    }
  };

  /** Add the device to the list without pairing (IP-direct): no code, no SAS,
   *  no trust — just reachable. */
  const doAdd = async () => {
    const s = live.current;
    if (s.busy || s.compare) return;
    const v = validateAddr(s.addr);
    if (!v.ok) {
      send({ type: "error", error: v.error });
      return;
    }
    send({ type: "addStart" });
    if (live.current.busy !== "add") return;
    const ep = live.current.epoch;
    try {
      const r = await api.connectByAddr(v.addr);
      await refreshDevices();
      const mine = live.current.epoch === ep;
      send({ type: "addDone", epoch: ep });
      if (!mine) return;
      showToast(t("devices.ipConnected", { name: r.name }));
      // Done — unless someone joined us meanwhile and a SAS is on screen.
      if (!live.current.compare) setPair(false);
    } catch (e) {
      send({
        type: "addDone",
        epoch: ep,
        error: joinErrorOf(e, v.addr, "devices.ipFailed"),
      });
    }
  };

  if (!pairOpen || !st.open) return null;

  const c = st.compare;
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
        style={{ width: MODAL_W.wide, fontFamily: "var(--font)" }}
        onClick={(e) => e.stopPropagation()}
      >
        {c ? (
          <ModalHead
            title={
              c.via === "host"
                ? t("pair.compareTitleHost", { name: c.name })
                : t("pair.compareTitleJoin", { name: c.name })
            }
            sub={t("pair.compareBody", { name: c.name })}
            onClose={close}
          />
        ) : (
          <>
            <ModalHead
              title={t("pair.title")}
              sub={t("pair.sub")}
              onClose={close}
            />
            <div style={{ padding: "14px 20px 0" }}>
              <Segmented
                options={[
                  { key: "host", label: t("pair.roleHost") },
                  { key: "join", label: t("pair.roleJoin") },
                ]}
                value={st.role}
                onChange={(role) => send({ type: "role", role })}
                itemStyle={{ flex: 1, padding: "6px 12px", fontSize: 12 }}
                disabled={!!st.busy}
              />
            </div>
          </>
        )}

        {c ? (
          <CompareView
            sas={c.sas}
            onMatch={acceptPeer}
            onMismatch={rejectPeer}
          />
        ) : st.role === "host" ? (
          <HostView
            st={st}
            remaining={remaining}
            expired={expired}
            onRegen={regen}
          />
        ) : (
          <JoinView
            st={st}
            send={send}
            onJoin={() => void doJoin()}
            onAdd={() => void doAdd()}
          />
        )}
        <div className="modal-foot">{t("pair.foot")}</div>
      </div>
    </div>
  );
}

/* ── shared bits ────────────────────────────────────────────────────────── */

const leadStyle: CSSProperties = {
  fontSize: 11.5,
  color: "var(--muted2)",
  lineHeight: 1.6,
};

const fieldLabel: CSSProperties = {
  fontSize: 10.5,
  fontWeight: 600,
  letterSpacing: ".07em",
  color: "var(--muted)",
};

/** An inline text action (换一个, 清除, …) — a real button, styled as a link. */
const linkBtn: CSSProperties = {
  border: "none",
  background: "none",
  padding: 0,
  font: "inherit",
  fontSize: 11,
  color: "var(--accent-ink)",
  cursor: "pointer",
};

function CopyButton({
  what,
  value,
  toast,
  disabled,
}: {
  /** What is copied, for the accessible name: 复制 · 本机地址. */
  what: string;
  value: string;
  toast: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const name = `${t("common.copy")} · ${what}`;
  return (
    <button
      type="button"
      className="btn ghost"
      title={name}
      aria-label={name}
      disabled={disabled}
      style={{
        height: 24,
        padding: "0 8px",
        fontSize: 11,
        color: disabled ? "var(--muted)" : "var(--accent-ink)",
        cursor: disabled ? "default" : "pointer",
        flex: "none",
      }}
      onClick={async () => {
        showToast((await copyText(value)) ? toast : t("common.copyFailed"));
      }}
    >
      {t("common.copy")}
    </button>
  );
}

function Card({
  label,
  flex,
  children,
}: {
  label: string;
  flex: string;
  children: ReactNode;
}) {
  return (
    <div
      style={{
        flex,
        minWidth: 0,
        border: "1px solid var(--border)",
        background: "var(--sidebar)",
        borderRadius: 10,
        padding: "9px 10px 9px 12px",
        display: "flex",
        flexDirection: "column",
        gap: 4,
      }}
    >
      <div style={fieldLabel}>{label}</div>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          minHeight: 30,
        }}
      >
        {children}
      </div>
    </div>
  );
}

/* ── 让对方加入我 ───────────────────────────────────────────────────────── */

function HostView({
  st,
  remaining,
  expired,
  onRegen,
}: {
  st: PairState;
  remaining: number;
  expired: boolean;
  onRegen: () => void;
}) {
  const { t } = useTranslation();
  const inv = st.invite;
  // `ip:port` splits at the last colon (IPv4 has none of its own).
  const cut = inv?.addr ? inv.addr.lastIndexOf(":") : -1;
  const ip = inv?.addr && cut > 0 ? inv.addr.slice(0, cut) : inv?.addr;
  const port = inv?.addr && cut > 0 ? inv.addr.slice(cut) : "";
  const preview = inv?.addr ? fmtLinkPreview(inv.qr) : "";
  // Something the other side can actually use is on screen.
  const usable = !!inv?.addr && !expired;

  return (
    <div
      style={{
        padding: "14px 20px 16px",
        display: "flex",
        flexDirection: "column",
        gap: 12,
        animation: "lbFade .18s ease",
      }}
    >
      <div style={leadStyle}>{t("pair.hostLead")}</div>

      <div style={{ display: "flex", gap: 10 }}>
        <Card label={t("pair.addrLabel")} flex="1.4">
          {inv && !inv.addr ? (
            <span style={{ fontSize: 11, color: "var(--danger)" }}>
              {t("pair.noLan")}
            </span>
          ) : (
            <>
              <span
                style={{
                  flex: 1,
                  minWidth: 0,
                  fontFamily: "var(--mono)",
                  fontSize: 14.5,
                  fontWeight: 600,
                  color: "var(--ink2)",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
              >
                {ip ?? "—"}
                <span style={{ color: "var(--muted)" }}>{port}</span>
              </span>
              {inv?.addr && (
                <CopyButton
                  what={t("pair.addrLabel")}
                  value={inv.addr}
                  toast={t("pair.addrCopied")}
                />
              )}
            </>
          )}
        </Card>
        <Card label={t("pair.codeLabel")} flex="1">
          <span
            data-testid="pair-code"
            style={{
              flex: 1,
              fontFamily: "var(--mono)",
              fontSize: 21,
              fontWeight: 600,
              letterSpacing: ".08em",
              color: expired ? "var(--muted)" : "var(--accent-ink)",
              textDecoration: expired ? "line-through" : "none",
              whiteSpace: "nowrap",
            }}
          >
            {inv ? fmtPairCode(inv.code) : "··· ···"}
          </span>
          {inv && (
            <CopyButton
              what={t("pair.codeLabel")}
              value={inv.code}
              toast={t("pair.codeCopied")}
              disabled={expired}
            />
          )}
        </Card>
      </div>

      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          marginTop: -4,
          fontSize: 11,
          color: expired ? "var(--danger)" : "var(--muted2)",
          minHeight: 16,
        }}
      >
        {inv && (expired || inv.expiresAt > 0) && (
          <span>
            {expired
              ? t("pair.expired")
              : t("pair.expiresIn", { t: fmtCountdown(remaining) })}
          </span>
        )}
        {(inv || st.mintFailed) && (
          <>
            {inv && (expired || inv.expiresAt > 0) && (
              <span style={{ color: "var(--muted)" }}>·</span>
            )}
            <button type="button" style={linkBtn} onClick={onRegen}>
              {t("pair.regen")}
            </button>
          </>
        )}
      </div>

      {usable && preview && inv && (
        <div
          style={{
            borderTop: "1px solid var(--border)",
            paddingTop: 12,
            display: "flex",
            flexDirection: "column",
            gap: 8,
          }}
        >
          <div style={leadStyle}>{t("pair.linkLead")}</div>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <code
              title={inv.qr}
              style={{
                flex: 1,
                minWidth: 0,
                fontFamily: "var(--mono)",
                fontSize: 10.5,
                color: "var(--muted)",
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {preview}
            </code>
            <button
              type="button"
              className="btn sm"
              style={{ flex: "none", height: 28, fontSize: 11.5 }}
              onClick={async () => {
                showToast(
                  (await copyText(inv.qr))
                    ? t("pair.linkCopied")
                    : t("common.copyFailed"),
                );
              }}
            >
              {t("pair.copyLink")}
            </button>
          </div>
        </div>
      )}

      {usable && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
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
      )}
    </div>
  );
}

/* ── 加入对方 ───────────────────────────────────────────────────────────── */

function JoinView({
  st,
  send,
  onJoin,
  onAdd,
}: {
  st: PairState;
  send: (a: PairAction) => void;
  onJoin: () => void;
  onAdd: () => void;
}) {
  const { t } = useTranslation();
  const addrRef = useRef<HTMLInputElement>(null);
  const codeRef = useRef<HTMLInputElement>(null);
  const joinRef = useRef<HTMLButtonElement>(null);
  const errId = "pair-join-error";
  const err = st.error;
  const busy = st.busy;
  const ready = !!st.addr.trim() && st.code.length === 6 && !busy;

  // Put the caret where the next keystroke belongs: the address when there is
  // none, else the code, else Join (a pasted link fills both — press Enter).
  // Re-runs when a link or prefill replaces the fields, not on every keystroke.
  useEffect(() => {
    const el = !st.addr.trim()
      ? addrRef.current
      : st.code.length < 6
        ? codeRef.current
        : joinRef.current;
    el?.focus();
  }, [st.source]);

  const fieldBorder = (field: "addr" | "code"): CSSProperties =>
    err?.field === field ? { borderColor: "var(--danger)" } : {};
  const onEnter = (e: ReactKeyboardEvent) => {
    if (e.key === "Enter" && !e.nativeEvent.isComposing) onJoin();
  };

  const src = st.source;
  const srcText = !src
    ? null
    : src.kind === "link"
      ? src.name
        ? t("pair.linkFrom", { name: src.name })
        : t("pair.linkLoaded")
      : src.name
        ? t("pair.addrFrom", { name: src.name })
        : t("pair.addrFromLink");

  return (
    <div
      style={{
        padding: "14px 20px 16px",
        display: "flex",
        flexDirection: "column",
        gap: 10,
        animation: "lbFade .18s ease",
      }}
    >
      <div style={leadStyle}>{t("pair.joinLead")}</div>

      <div style={{ display: "flex", gap: 8, alignItems: "flex-end" }}>
        <label
          style={{
            flex: 1,
            minWidth: 0,
            display: "flex",
            flexDirection: "column",
            gap: 5,
          }}
        >
          <span style={fieldLabel}>{t("pair.joinAddr")}</span>
          <input
            ref={addrRef}
            className="input mono"
            style={{ fontSize: 12, ...fieldBorder("addr") }}
            value={st.addr}
            placeholder={t("pair.joinAddrPh")}
            spellCheck={false}
            autoComplete="off"
            readOnly={!!busy}
            aria-invalid={err?.field === "addr"}
            aria-describedby={err ? errId : undefined}
            onChange={(e) => send({ type: "addr", value: e.target.value })}
            onKeyDown={onEnter}
          />
        </label>
        <label
          style={{
            width: 104,
            flex: "none",
            display: "flex",
            flexDirection: "column",
            gap: 5,
          }}
        >
          <span style={fieldLabel}>{t("pair.joinCode")}</span>
          <input
            ref={codeRef}
            className="input mono"
            style={{
              fontSize: 13,
              // Space the digits like the host shows them — not the placeholder.
              letterSpacing: st.code ? ".08em" : undefined,
              ...fieldBorder("code"),
            }}
            value={fmtPairCode(st.code)}
            placeholder={t("pair.joinCodePh")}
            inputMode="numeric"
            spellCheck={false}
            autoComplete="one-time-code"
            readOnly={!!busy}
            aria-invalid={err?.field === "code"}
            aria-describedby={err ? errId : undefined}
            onChange={(e) => send({ type: "code", value: e.target.value })}
            onKeyDown={onEnter}
          />
        </label>
        <button
          ref={joinRef}
          type="button"
          className={`btn primary${ready ? "" : " off"}`}
          style={{ flex: "none", height: 30, padding: "0 14px" }}
          disabled={!!busy}
          onClick={onJoin}
        >
          {busy === "join" ? t("pair.joining") : t("pair.joinBtn")}
        </button>
      </div>

      {err && (
        <div
          id={errId}
          role="alert"
          style={{ fontSize: 11, lineHeight: 1.55, color: "var(--danger)" }}
        >
          {t(err.key, err.params)}
        </div>
      )}

      {srcText ? (
        <div
          style={{
            alignSelf: "flex-start",
            display: "inline-flex",
            alignItems: "center",
            gap: 8,
            maxWidth: "100%",
            fontSize: 11,
            color: "var(--accent-ink)",
            background: "var(--accent-soft)",
            borderRadius: 99,
            padding: "3px 6px 3px 11px",
          }}
        >
          <span
            style={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {srcText}
          </span>
          <button
            type="button"
            style={{ ...linkBtn, color: "var(--muted2)", padding: "0 5px" }}
            disabled={!!busy}
            onClick={() => send({ type: "clearSource" })}
          >
            {t("pair.linkClear")}
          </button>
        </div>
      ) : (
        <div style={{ fontSize: 10.5, color: "var(--muted)" }}>
          {t("pair.joinPasteHint")}
        </div>
      )}

      <div
        style={{
          borderTop: "1px solid var(--border)",
          paddingTop: 10,
          display: "flex",
          alignItems: "baseline",
          gap: 8,
          flexWrap: "wrap",
        }}
      >
        <button
          type="button"
          style={{
            ...linkBtn,
            color: busy ? "var(--muted)" : "var(--accent-ink)",
            cursor: busy ? "default" : "pointer",
          }}
          disabled={!!busy}
          onClick={onAdd}
        >
          {busy === "add" ? t("pair.adding") : t("pair.addOnly")}
        </button>
        <span style={{ fontSize: 10.5, color: "var(--muted)" }}>
          {t("pair.addOnlyHint")}
        </span>
      </div>
    </div>
  );
}

/* ── compare: the code was redeemed, nothing is trusted yet ─────────────── */

function CompareView({
  sas,
  onMatch,
  onMismatch,
}: {
  sas: string;
  onMatch: () => void;
  onMismatch: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 8,
        padding: "18px 20px 18px",
        animation: "lbFade .2s ease",
      }}
    >
      <div style={fieldLabel}>{t("pair.compareTitle")}</div>
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
        {fmtSas(sas)}
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
      <div style={{ display: "flex", gap: 8, marginTop: 6, width: "100%" }}>
        <button
          type="button"
          className="btn danger"
          style={{ flex: "none", padding: "0 14px" }}
          onClick={onMismatch}
        >
          {t("pair.mismatch")}
        </button>
        <button
          type="button"
          className="btn primary"
          style={{ flex: 1 }}
          onClick={onMatch}
        >
          {t("pair.match")}
        </button>
      </div>
    </div>
  );
}
