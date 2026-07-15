import { useEffect, useRef, useState } from "react";
import { Outlet, useNavigate } from "react-router-dom";
import i18n from "../i18n";
import Sidebar from "./Sidebar";
import Header from "./Header";
import Toast from "./Toast";
import DragOverlay from "./DragOverlay";
import SendModal from "./SendModal";
import IncomingStack from "./IncomingStack";
import TransferDetail from "./TransferDetail";
import PairModal from "./PairModal";
import QuickTextModal from "./QuickTextModal";
import ShareModal from "./ShareModal";
import LicenseModal from "./LicenseModal";
import FpAlertModal from "./FpAlertModal";
import ConflictModal from "./ConflictModal";
import InputContextMenu from "./InputContextMenu";
import * as api from "../bridge/api";
import type {
  DiscoveredDevice,
  IncomingRequest,
  NetDegradedEvent,
  SasEvent,
  ShareDownloadEvent,
  ShareEntry,
  TextReceivedEvent,
  TransferEvent,
  TransferFileProgressEvent,
  TransferFileDoneEvent,
  TrustedPeer,
} from "../bridge/api";
import {
  inboxFromText,
  inboxFromTransfer,
  isTrustMigrating,
  notify,
  resolvedTheme,
  setVisibility,
  shortFp,
  showToast,
  syncTrustFromBackend,
  useData,
  useInbox,
  useOverlays,
  usePrefs,
  useShares,
  useSysDark,
  useToast,
  useTransfers,
  useTrust,
  useRecents,
  visibilityOf,
  displayIp,
  sendFileFromPath,
} from "../lib/store";
import { maybeSeedDemo } from "../lib/demo";
import { suppressBrowserShortcuts } from "../lib/browserShortcuts";
import { installZoomHotkeys } from "../lib/uiZoom";
import { parseDeepLink } from "../lib/deepLink";
import {
  errText,
  openDir,
  sendToDevice,
  transferErrText,
} from "../lib/sendops";

/** Display name for a peer: the user's own rename wins, then the name the
 *  sender declared for this very session (M4.2), then discovery, then the
 *  fingerprint — never blank. */
function peerNameOf(deviceId: string, senderName?: string): string {
  const rec = useTrust.getState().records[deviceId];
  if (rec?.name) return rec.name;
  if (senderName) return senderName;
  const dev = useData.getState().devices.find((d) => d.deviceId === deviceId);
  return dev?.name ?? shortFp(deviceId);
}

/** Do the only three things a `lanbeam://` link is ALLOWED to do: surface (the
 *  backend already did that), PRE-FILL, or NAVIGATE — never act. A deep link is
 *  untrusted: any web page can ask the OS to open one. See `lib/deepLink.ts` for
 *  the contract; an unknown command is dropped rather than guessed at. */
function routeDeepLink(raw: string, navigate: (path: string) => void): void {
  const link = parseDeepLink(raw);
  if (!link) return;
  const ov = useOverlays.getState();
  switch (link.cmd) {
    case "pair":
      // Pre-fills the join field; the user still confirms the SAS.
      ov.setPair(true, link.url);
      break;
    case "text":
      // Pre-fills the body; the user still picks a device and presses send.
      ov.setQt(true, link.text);
      break;
    case "connect":
      // Stages the address on the Devices page. It does NOT dial.
      ov.setConnectPrefill(link.addr);
      navigate("/devices");
      break;
    case "devices":
    case "transfers":
    case "inbox":
    case "settings":
      navigate(`/${link.cmd}`);
      break;
    case "open":
      // The window is already up — that was the whole request.
      break;
  }
}

/** Push the tray's whole localized snapshot. Reads live state through getState(),
 *  so an event handler can call it too — muda flips a CheckMenuItem's tick
 *  OPTIMISTICALLY on click, and only a re-push can un-lie it if the toggle then
 *  fails.
 *
 *  A NO-OP until settings have loaded. Rust builds the tray from the REAL
 *  persisted values, so pushing a fabricated "nothing loaded yet" snapshot
 *  (discoverable=false, no LAN address) would overwrite the truth with a guess —
 *  permanently so, if `load()` never succeeds. Say nothing rather than lie. */
function pushTray(lang: string): void {
  const { settings, networkInfo } = useData.getState();
  if (!settings) return;
  // Pin the language: t() reads i18n's live state, which React cannot see.
  const L = (k: string) => i18n.t(k, { lng: lang });
  const name = settings.deviceName || "LanBeam";
  const ip = displayIp(networkInfo, usePrefs.getState().iface);
  void api
    .syncTray({
      status: `${name} · ${ip ?? L("tray.noLan")}`,
      tooltip: `LanBeam · ${name}`,
      show: L("tray.show"),
      send: L("tray.send"),
      quickText: L("tray.quickText"),
      share: L("tray.share"),
      pair: L("tray.pair"),
      discoverable: L("tray.discoverable"),
      openDir: L("tray.openDir"),
      inbox: L("tray.inbox"),
      transfers: L("tray.transfers"),
      settings: L("tray.settings"),
      quit: L("tray.quit"),
      isDiscoverable: settings.discoverable,
    })
    .catch(() => {});
}

export default function AppShell() {
  const themeMode = usePrefs((s) => s.themeMode);
  const sysDark = useSysDark((s) => s.dark);
  const loadData = useData((s) => s.load);
  const navigate = useNavigate();
  // The Tauri event listeners below live for the component's lifetime ([] deps)
  // so a route change never tears down and re-registers them — onEvent's async
  // unlisten racing a fresh listen() could otherwise drop an event landing in
  // the gap. They read navigate through a ref kept current instead of closing
  // over the render-scoped value (which changes identity on every navigation).
  const navRef = useRef(navigate);
  useEffect(() => {
    navRef.current = navigate;
  });

  // Pull identity + settings from the backend once at startup.
  useEffect(() => {
    loadData();
    maybeSeedDemo();
  }, [loadData]);

  // Trust store: one-time legacy localStorage import, then hydrate from the
  // backend (M4.4). `trust_updated` below keeps it live afterwards.
  useEffect(() => {
    void syncTrustFromBackend();
  }, []);

  // Apply 历史记录保留 to whatever just rehydrated from localStorage. The write
  // side (partialize) can only trim on the NEXT write, so a row that expired
  // while the app was closed would otherwise sit in the list until something
  // else happened to persist.
  useEffect(() => {
    useTransfers.getState().pruneHistory();
  }, []);

  // Live browser shares: hydrate once, then follow the backend. A live share is
  // files being served over HTTP on the LAN, and the sidebar has to keep saying
  // so for exactly as long as that is true — including when the share expires on
  // its own, which is why the backend broadcasts from its sweeper too.
  useEffect(() => {
    void useShares.getState().load();
  }, []);
  useEffect(
    () =>
      api.onEvent<ShareEntry[]>("shares_updated", (list) => {
        useShares.getState().setShares(list);
      }),
    [],
  );

  // Every backend trust mutation broadcasts the full list.
  useEffect(
    () =>
      api.onEvent<TrustedPeer[]>("trust_updated", (list) => {
        // During the one-time legacy import each set_trusted emits a
        // partial-so-far list; hydrating it would erase not-yet-imported
        // records (and their circle positions). syncTrustFromBackend ends
        // with one authoritative hydrate instead.
        if (isTrustMigrating()) return;
        useTrust.getState().hydrate(list);
      }),
    [],
  );

  // Silent network degradations (M4.6): the backend still works in a reduced
  // mode, but the user must learn WHY peers can't see them / the port moved.
  // Both bind-time fallbacks fire during setup(), before this listener exists
  // and Tauri events have no replay — pull the recorded backlog once, and
  // keep the live listener for anything that degrades after startup.
  useEffect(() => {
    const toastDegraded = (p: NetDegradedEvent) => {
      if (p.kind === "udp_recv_fallback")
        showToast(i18n.t("net.udpDegraded"), null, 6000);
      else if (p.kind === "tcp_port_fallback")
        showToast(i18n.t("net.tcpFallback"), null, 6000);
      // Unknown future kinds are already logged backend-side; no raw
      // untranslated detail strings in the UI.
    };
    void api
      .getNetStatus()
      .then((list) => list.forEach(toastDegraded))
      .catch(() => {});
    return api.onEvent<NetDegradedEvent>("net_degraded", toastDegraded);
  }, []);

  // Resolve light/dark (incl. system) onto <html data-theme>.
  useEffect(() => {
    document.documentElement.setAttribute(
      "data-theme",
      resolvedTheme(themeMode, sysDark),
    );
  }, [themeMode, sysDark]);

  // Track the OS color scheme for "system" mode.
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!mq) return;
    const fn = (e: MediaQueryListEvent) => useSysDark.getState().set(e.matches);
    mq.addEventListener("change", fn);
    return () => mq.removeEventListener("change", fn);
  }, []);

  // Ghost mode auto-restore (临时隐身 1 小时后恢复).
  useEffect(() => {
    const check = () => {
      const { ghostUntil } = usePrefs.getState();
      const settings = useData.getState().settings;
      if (
        ghostUntil &&
        Date.now() >= ghostUntil &&
        visibilityOf(settings, ghostUntil) !== "on"
      ) {
        // setVisibility rolls back + rethrows on failure; the 30 s interval
        // retries, so a failed auto-restore is safely swallowed here.
        void setVisibility("on").catch(() => {});
      }
    };
    check();
    const id = setInterval(check, 30_000);
    return () => clearInterval(id);
  }, []);

  // Strip the browser out of the WebView: Ctrl+F's find bar, Ctrl+P, Ctrl+S,
  // Ctrl+U… none of that belongs in a packaged desktop app. See the module for
  // why this is a keydown swallow rather than a WebView2 flag.
  useEffect(() => suppressBrowserShortcuts(), []);

  // Ctrl +/-/0 drive the app's OWN scale setting. The webview's native zoom hotkeys
  // are off (browser chrome doesn't belong in a packaged app) — but a desktop app
  // with a scale setting and no Ctrl +/-/0 is just missing its front door.
  useEffect(() => installZoomHotkeys(), []);

  // Native menu → "About" lands on the settings page (关于 section).
  useEffect(
    () => api.onEvent("menu:about", () => navRef.current("/settings")),
    [],
  );

  // Global shortcut (M5.5): the backend already showed + focused the window;
  // the webview's half is opening the quick-text panel.
  useEffect(
    () =>
      api.onEvent("hotkey:quick-text", () =>
        useOverlays.getState().setQt(true),
      ),
    [],
  );

  /* ── system tray (M5.3+) ──────────────────────────────────────────────────
     The tray is a REMOTE: the backend owns only show/quit, and has neither an
     i18n layer nor the app state, so its whole user-facing surface is pushed
     from here and every other item comes back as a `tray:*` event we service
     with the SAME actions the header/pages use. One writer per piece of state,
     no duplicated logic. */

  // i18n mutates in place, so React needs an explicit nudge to re-push labels
  // after a language switch.
  const [lang, setLang] = useState(i18n.language);
  useEffect(() => {
    const onLang = (l: string) => setLang(l);
    i18n.on("languageChanged", onLang);
    return () => {
      i18n.off("languageChanged", onLang);
    };
  }, []);

  const trayName = useData((s) => s.settings?.deviceName ?? "");
  const trayIface = usePrefs((s) => s.iface);
  // Select the DERIVED IP STRING, never the networkInfo ARRAY: every focus-time
  // refresh hands the store a fresh array, and subscribing to it would re-render
  // this root component (and the whole tree) for an IP that did not change.
  const trayIp = useData((s) => displayIp(s.networkInfo, trayIface) ?? "");
  const trayDiscoverable = useData((s) => !!s.settings?.discoverable);
  // Until settings load we know NOTHING — see pushTray.
  const trayLoaded = useData((s) => !!s.settings);
  useEffect(() => {
    pushTray(lang);
  }, [trayName, trayIp, trayDiscoverable, trayLoaded, lang]);

  // Tray items → the same actions the UI already exposes. Rust surfaced the
  // window first for anything that shows something; the rest work headless.
  useEffect(() => {
    const offs = [
      api.onEvent("tray:send", () => {
        if (!useData.getState().devices.length) {
          showToast(i18n.t("devices.noDevicesToast"));
          return;
        }
        useOverlays.getState().openSend(null, useRecents.getState().items);
      }),
      api.onEvent("tray:quick_text", () => useOverlays.getState().setQt(true)),
      api.onEvent("tray:share", () => useOverlays.getState().setShare(true)),
      api.onEvent("tray:pair", () => useOverlays.getState().setPair(true)),
      api.onEvent("tray:inbox", () => navRef.current("/inbox")),
      api.onEvent("tray:transfers", () => navRef.current("/transfers")),
      api.onEvent("tray:settings", () => navRef.current("/settings")),
      api.onEvent("tray:open_dir", () => {
        void (async () => {
          // The store's copy may not have loaded yet. FETCH it rather than
          // silently doing nothing — a menu entry that no-ops on click is
          // exactly the failure this menu exists to avoid.
          const d = useData.getState();
          const dir =
            d.downloadDir || (await api.getDownloadDir().catch(() => ""));
          if (!dir) {
            showToast(i18n.t("errors.generic"));
            return;
          }
          // Surface the REAL reason (missing folder vs failed open) — a blanket
          // "something went wrong" is what hid this bug in the first place.
          await openDir(dir).catch((e) => showToast(errText(e)));
        })();
      }),
      // The tick toggles the SAME visibility the sidebar owns — setVisibility
      // (not raw setDiscoverable) so the 隐身 timer is cleared too.
      api.onEvent("tray:discoverable", () => {
        const s = useData.getState().settings;
        // muda already flipped the tick optimistically. If we don't yet know the
        // truth (settings unloaded), or the toggle below rolls back, re-push —
        // otherwise the tray is left wearing a tick that isn't real.
        if (!s) {
          pushTray(i18n.language);
          return;
        }
        void setVisibility(s.discoverable ? "off" : "on").catch(() => {
          pushTray(i18n.language);
        });
      }),
    ];
    return () => {
      for (const off of offs) off();
    };
  }, []);

  // Deep link (lanbeam://…) captured by the backend while we were running.
  useEffect(
    () =>
      api.onEvent<string>("deep_link", (url) =>
        routeDeepLink(url, navRef.current),
      ),
    [],
  );

  // Cold start: the app may have been LAUNCHED by a lanbeam:// link before that
  // listener existed (Tauri events have no replay), so the backend stashed it.
  // Pull it once on mount and route it identically.
  useEffect(() => {
    void api
      .takePendingDeepLink()
      .then((url) => {
        if (url) routeDeepLink(url, navRef.current);
      })
      .catch(() => {});
  }, []);

  // Live device list pushed from the discovery service.
  useEffect(
    () =>
      api.onEvent<DiscoveredDevice[]>("devices_updated", (devices) => {
        useData.getState().setDevices(devices);
        const trust = useTrust.getState();
        for (const d of devices)
          if (trust.records[d.deviceId]) trust.touch(d.deviceId, d.name);
      }),
    [],
  );

  // Incoming request. The accept-or-prompt decision moved into the backend
  // (M4.4): `autoAccepted: true` means the receive policy already said yes and
  // no reply is expected — replying would go nowhere. The UI only narrates it.
  useEffect(
    () =>
      api.onEvent<IncomingRequest>("incoming_file_request", (r) => {
        const peerName = peerNameOf(r.deviceId, r.senderName);
        if (r.autoAccepted) {
          // Auto-accept answered "do I want this", not "what about my existing
          // file". If the user set 冲突策略 = 每次询问, that question is still
          // theirs — the backend parks a prompt and waits for this reply. (It
          // used to answer "rename" for them on exactly this path, which under
          // the default settings is the ONLY path, so the modal never appeared.)
          if (r.conflicts?.length && r.conflictPolicy === "ask") {
            useOverlays
              .getState()
              .setConflict({ request: r, peerName, wantTrust: false });
          } else {
            useTransfers.getState().acceptMeta(r, peerName);
            showToast(i18n.t("incoming.acceptToast", { name: peerName }));
          }
        } else {
          useTransfers.getState().pushIncoming(r);
        }
        notify();
      }),
    [],
  );

  // Quick text arrived (M7.3): drop a text item into the inbox, toast who sent
  // it with a 查看 action that jumps there. Whether it was also placed on this
  // machine's clipboard is the backend's call (gated on the local clip_share
  // consent), so the inbox 已入剪贴板 pill stays a manual-copy indicator here.
  useEffect(
    () =>
      api.onEvent<TextReceivedEvent>("text_received", (p) => {
        const name = peerNameOf(p.deviceId, p.senderName);
        useInbox.getState().add(inboxFromText(name, p.text, p.at));
        // Also log it in the unified transfer history (历史) — the inbox stays
        // the content store; this is the lightweight「我收到的」row. EXCEPT for a
        // send-to-self loopback: sendTextTracked already logged the "send" row,
        // so skip the mirrored "receive" row (matches the file-loopback dedup)
        // rather than showing one message as two history rows.
        if (p.deviceId !== useData.getState().identity?.deviceId) {
          useTransfers.getState().addTextTransfer({
            direction: "receive",
            peerId: p.deviceId,
            peerName: name,
            text: p.text,
          });
        }
        showToast(
          i18n.t("inbox.textToast", { name }),
          {
            label: i18n.t("common.view"),
            fn: () => {
              navRef.current("/inbox");
              useToast.getState().hide();
            },
          },
          5000,
        );
        notify();
      }),
    [],
  );

  // A browser pulled a shared file (M8.4). Surface it: a toast (what + who) and
  // a persistent「传输·历史」row. The OS notification is fired backend-side, so
  // it works even when the window is closed to tray.
  useEffect(
    () =>
      api.onEvent<ShareDownloadEvent>("share_download", (p) => {
        useTransfers.getState().addShareDownload({
          name: p.name,
          size: p.size,
          peerIp: p.peerIp,
        });
        showToast(
          i18n.t("share.downloadedToast", { name: p.name, ip: p.peerIp }),
          {
            label: i18n.t("common.view"),
            fn: () => {
              navRef.current("/transfers");
              useToast.getState().hide();
            },
          },
          5000,
        );
      }),
    [],
  );

  // SAS issued for an outgoing session (before the peer accepts).
  useEffect(
    () =>
      api.onEvent<SasEvent>("sas_code", (p) => {
        // connect_device emits sas_code without a sessionId — no transfer.
        if (!p.sessionId) return;
        useTransfers.getState().attachSas(p.sessionId, p.sas, p.deviceId);
      }),
    [],
  );

  useEffect(
    () =>
      api.onEvent<TransferEvent>("transfer_started", (p) => {
        useTransfers.getState().upsert({
          sessionId: p.sessionId,
          direction: p.direction,
          totalSize: p.totalSize,
          fileCount: p.fileCount,
          status: "active",
          started: true,
        });
      }),
    [],
  );

  // A transfer parked on the concurrency gate (M6.7): mark the row "queued" so
  // it reads as waiting-for-a-slot, not stuck at 0%. The subsequent
  // transfer_started (send: queued→started; receive: the first progress tick)
  // flips it back to "active" on its own, so no explicit un-queue is needed.
  useEffect(
    () =>
      api.onEvent<TransferEvent>("transfer_queued", (p) => {
        useTransfers.getState().upsert({
          sessionId: p.sessionId,
          direction: p.direction,
          status: "queued",
        });
      }),
    [],
  );

  useEffect(
    () =>
      api.onEvent<TransferEvent>("transfer_progress", (p) => {
        useTransfers
          .getState()
          .progress(p.sessionId, p.percent ?? 0, p.total ?? p.totalSize);
      }),
    [],
  );

  // Per-file progress + completion (M6.8): the detail drawer's per-file rows
  // read these for real bars and verified ticks (they fall back to a
  // cumulative-size estimate for a session with no per-file events yet).
  useEffect(
    () =>
      api.onEvent<TransferFileProgressEvent>("transfer_file_progress", (p) => {
        useTransfers
          .getState()
          .fileProgress(p.sessionId, p.fileIndex, p.percent);
      }),
    [],
  );

  useEffect(
    () =>
      api.onEvent<TransferFileDoneEvent>("transfer_file_done", (p) => {
        useTransfers.getState().fileDone(p.sessionId, p.fileIndex, p.verified);
      }),
    [],
  );

  useEffect(
    () =>
      api.onEvent<TransferEvent>("transfer_done", (p) => {
        const st = useTransfers.getState();
        st.upsert({
          sessionId: p.sessionId,
          savedNames: p.savedNames,
          status: "done",
          percent: 100,
        });
        const tr = useTransfers.getState().transfers[p.sessionId];
        if (!tr) return;
        const name = tr.name ?? tr.savedNames?.[0] ?? "";
        // Branch on the EVENT's direction, not the merged store entry: a
        // send-to-self (loopback) reuses one transfer_id as the sessionId for
        // BOTH the send and receive sides, so `tr.direction` gets overwritten to
        // "receive" and the send's own transfer_done would wrongly add a second
        // inbox record. The per-event `p.direction` is authoritative.
        if (p.direction === "receive") {
          showToast(
            i18n.t("transfers.recvToast", { peer: tr.peerName ?? "", name }),
            {
              label: i18n.t("common.view"),
              fn: () => {
                navRef.current("/inbox");
                useToast.getState().hide();
              },
            },
            5000,
          );
          void api.revealReceived(p.sessionId).then((paths) => {
            useInbox.getState().add(inboxFromTransfer(tr, paths));
          });
        } else {
          showToast(
            i18n.t("transfers.sentToast", { peer: tr.peerName ?? "", name }),
          );
        }
        notify();
      }),
    [],
  );

  useEffect(
    () =>
      api.onEvent<TransferEvent>("transfer_error", (p) => {
        const st = useTransfers.getState();
        // A session that errors while its prompt card is still up (the
        // backend's 120s accept window expired) must drop the card, or a
        // later Accept click becomes a silent no-op on a dead session.
        st.removeIncoming(p.sessionId);
        // A parked ConflictModal for this very session is now stale too: if the
        // backend declined/timed it out while the user deliberated, a later
        // keep-both/overwrite click would reply on a dead session (backend
        // no-op) and falsely toast success. Drop the overlay before the
        // unknown-session early-out below (tr is undefined in that scenario).
        if (useOverlays.getState().conflict?.request.sessionId === p.sessionId)
          useOverlays.getState().setConflict(null);
        // Declined/timed-out/unsafe incoming requests error out sessions the
        // UI never tracked — don't fabricate junk history rows for them.
        const tr = st.transfers[p.sessionId];
        if (!tr) return;
        // The peer politely saying no (M4.5 `code`) is an answer, not a
        // failure: toast it and drop the row instead of keeping error history.
        if (p.code === "declined" && tr.direction === "send") {
          st.removeTransfer(p.sessionId);
          showToast(i18n.t("send.declinedToast"));
          return;
        }
        st.upsert({
          sessionId: p.sessionId,
          status: "error",
          error: p.error,
          errorCode: p.code,
        });
        // Say it out loud. Success gets a toast AND an OS notification; failure
        // used to get a row quietly turning red on a page nobody was looking at
        // — so a receive that ran for ten minutes and then deleted a corrupt
        // file did the whole thing in silence.
        showToast(
          i18n.t("transfers.failedToast", {
            peer: tr.peerName ?? "",
            err: transferErrText(p.code),
          }),
          {
            label: i18n.t("common.view"),
            fn: () => {
              navRef.current("/transfers");
              useToast.getState().hide();
            },
          },
          6000,
        );
        notify();
      }),
    [],
  );

  // The backend's pause is BOUNDED: it parks the byte loop, and when the park
  // expires it resumes on its own and says so. Nothing listened for that, so the
  // row sat at 「已暂停」 forever while the progress bar crept along underneath
  // it — the UI insisting on a state the backend had already left. (Rust has
  // been emitting this event, and commenting that it emits it, the whole time.)
  useEffect(
    () =>
      api.onEvent<TransferEvent>("transfer_resumed", (p) => {
        useTransfers.getState().setPaused(p.sessionId, false);
      }),
    [],
  );

  // OS drag-drop (Tauri gives real paths + cursor position for hit-testing).
  useEffect(() => {
    if (!api.isTauri) return;
    let un: (() => void) | undefined;
    let disposed = false;
    void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) => {
      if (disposed) return;
      void getCurrentWebview()
        .onDragDropEvent((ev) => {
          const ov = useOverlays.getState();
          const pl = ev.payload;
          if (pl.type === "enter" || pl.type === "over") {
            const k = window.devicePixelRatio || 1;
            const el = document.elementFromPoint(
              pl.position.x / k,
              pl.position.y / k,
            );
            const host = el?.closest?.(
              "[data-device-id]",
            ) as HTMLElement | null;
            ov.setDrag(true, host?.dataset.deviceId ?? null);
          } else if (pl.type === "drop") {
            const files = pl.paths.map(sendFileFromPath);
            const targetId = useOverlays.getState().dragDevice;
            ov.setDrag(false);
            if (!files.length) return;
            const dev = useData
              .getState()
              .devices.find((d) => d.deviceId === targetId);
            if (dev) {
              if (sendToDevice(dev, files))
                showToast(
                  i18n.t("devices.startSendToast", {
                    n: files.length,
                    name: dev.name,
                  }),
                );
            } else {
              ov.openSend(null, useRecents.getState().items, files);
              ov.patchSend({ step: "device" });
            }
          } else {
            ov.setDrag(false);
          }
        })
        .then((off) => {
          // Cleanup may have run while registration was in flight.
          if (disposed) off();
          else un = off;
        });
    });
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  return (
    <div
      style={{
        position: "relative",
        display: "flex",
        height: "100vh",
        overflow: "hidden",
        background: "var(--bg)",
        color: "var(--ink)",
        fontFamily: "var(--font)",
        fontSize: 13,
        lineHeight: 1.5,
        transition: "background .25s ease,color .25s ease",
      }}
    >
      <Sidebar />
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          minWidth: 0,
        }}
      >
        <Header />
        <Outlet />
      </div>
      <TransferDetail />
      <DragOverlay />
      <SendModal />
      <IncomingStack />
      <PairModal />
      <QuickTextModal />
      <ShareModal />
      <LicenseModal />
      <FpAlertModal />
      <ConflictModal />
      <Toast />
      <InputContextMenu />
    </div>
  );
}
