/** 收件箱 — received files & texts grouped by day, with hover actions,
 *  multi-select batch bar and a right-click context menu (+ forward submenu). */
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { isTauri, type DiscoveredDevice } from "../bridge/api";
import {
  resolvedTheme,
  sendFileFromPath,
  showToast,
  useData,
  useInbox,
  usePrefs,
  useSysDark,
  type InboxItem,
} from "../lib/store";
import { fmtBytes, fmtWhen, whenGroup } from "../lib/format";
import {
  copyText,
  errText,
  isNotFound,
  openDir,
  openFile,
  revealFile,
  sendTextTracked,
  sendToDevice,
} from "../lib/sendops";
import { ExtChip, Segmented } from "../components/ui";

type IbFilter = "all" | "img" | "vid" | "doc" | "txt";

type MenuState = {
  id: string;
  mode: "fwd" | "full";
  x: number;
  y: number;
  sub: boolean;
};

type MenuEntry =
  | { sep: true }
  | {
      sep?: undefined;
      label: string;
      fg?: string;
      note?: string;
      dot?: string;
      hasSub?: boolean;
      onClick?: () => void;
    };

/** 26px bordered hover action chip shown on row hover. */
function ActionChip({
  label,
  fg,
  onClick,
}: {
  label: string;
  fg: string;
  onClick: (e: ReactMouseEvent<HTMLSpanElement>) => void;
}) {
  return (
    // biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); it is keyboard-operable via role + tabIndex + onKeyDown
    <span
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        if (e.key === " ") e.preventDefault();
        e.stopPropagation();
        const r = e.currentTarget.getBoundingClientRect();
        onClick({
          clientX: r.left + r.width / 2,
          clientY: r.top + r.height / 2,
          preventDefault: () => {},
          stopPropagation: () => {},
        } as unknown as ReactMouseEvent<HTMLSpanElement>);
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = "var(--hover)")}
      onMouseLeave={(e) => (e.currentTarget.style.background = "var(--panel)")}
      style={{
        height: 26,
        display: "inline-flex",
        alignItems: "center",
        padding: "0 10px",
        borderRadius: 7,
        border: "1px solid var(--border2)",
        background: "var(--panel)",
        fontSize: 10.5,
        fontWeight: 600,
        color: fg,
        cursor: "pointer",
        boxSizing: "border-box",
      }}
    >
      {label}
    </span>
  );
}

const menuPanelStyle: CSSProperties = {
  position: "fixed",
  zIndex: 61,
  width: 190,
  background: "var(--panel)",
  border: "1px solid var(--border2)",
  borderRadius: 10,
  boxShadow: "var(--shadow)",
  padding: 5,
  boxSizing: "border-box",
  animation: "lbFade .12s ease",
};

type TFn = ReturnType<typeof useTranslation>["t"];

/** A single inbox row. Owns its own `hovered` state so pointer sweeps only
 *  re-render the row under the cursor — not the whole page (which would re-run
 *  the full-list group/filter passes on every mouseenter/mouseleave). Wrapped
 *  in memo with scalar props so unrelated page state (menu/selection) changes
 *  only re-render the affected rows. */
const InboxRow = memo(function InboxRow({
  it,
  name,
  checked,
  selMode,
  menuOpen,
  copied,
  t,
  onToggleSel,
  onOpenMenu,
  onCopy,
  onOpen,
  onReveal,
}: {
  it: InboxItem;
  name: string;
  checked: boolean;
  selMode: boolean;
  menuOpen: boolean;
  copied: boolean;
  t: TFn;
  onToggleSel: (id: string) => void;
  onOpenMenu: (e: ReactMouseEvent, id: string, mode: "fwd" | "full") => void;
  onCopy: (it: InboxItem) => void;
  onOpen: (it: InboxItem) => void;
  onReveal: (it: InboxItem) => void;
}) {
  const [hovered, setHovered] = useState(false);
  const isTxt = it.kind === "txt";
  const showCk = selMode || hovered;
  const showActs = hovered && !selMode;
  const sizeStr = isTxt
    ? t("inbox.charCount", { n: (it.text ?? "").length })
    : fmtBytes(it.sizeBytes);
  return (
    // biome-ignore lint/a11y/useSemanticElements: keep the row as a div (a native <button> can't nest the checkbox/action controls and would restyle it); keyboard-operable via role + tabIndex + onKeyDown
    <div
      role="button"
      tabIndex={0}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onContextMenu={(e) => onOpenMenu(e, it.id, "full")}
      onClick={(e) => {
        if (selMode || e.metaKey || e.ctrlKey) onToggleSel(it.id);
      }}
      onKeyDown={(e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        if (e.key === " ") e.preventDefault();
        if (selMode || e.metaKey || e.ctrlKey) onToggleSel(it.id);
      }}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 11,
        padding: "10px 14px",
        borderBottom: "1px solid var(--border)",
        transition: "background .15s ease",
        background: checked
          ? "var(--accent-soft)"
          : menuOpen || hovered
            ? "var(--hover)"
            : "transparent",
        cursor: selMode ? "pointer" : "default",
      }}
    >
      {showCk ? (
        // biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown
        <span
          role="button"
          tabIndex={0}
          onClick={(e) => {
            e.stopPropagation();
            onToggleSel(it.id);
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter" && e.key !== " ") return;
            if (e.key === " ") e.preventDefault();
            e.stopPropagation();
            onToggleSel(it.id);
          }}
          style={{
            width: 30,
            height: 30,
            flex: "none",
            display: "grid",
            placeItems: "center",
            cursor: "pointer",
          }}
        >
          <span
            style={{
              width: 16,
              height: 16,
              borderRadius: 5,
              boxSizing: "border-box",
              border: checked ? "none" : "1.5px solid var(--border2)",
              background: checked ? "var(--accent)" : "var(--panel)",
              color: "var(--accent-fg)",
              display: "grid",
              placeItems: "center",
              fontSize: 10,
              fontWeight: 700,
              lineHeight: 1,
            }}
          >
            {checked ? "✓" : ""}
          </span>
        </span>
      ) : (
        <ExtChip ext={it.ext} size={30} radius={8} fontSize={8} isTxt={isTxt} />
      )}
      <span style={{ flex: 1, minWidth: 0 }}>
        <span
          style={{
            display: "block",
            fontSize: 12,
            fontWeight: 600,
            color: "var(--ink2)",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {name}
        </span>
        <span
          style={{
            display: "block",
            fontSize: 10.5,
            color: "var(--muted)",
            marginTop: 1,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {t("inbox.from", {
            from: it.from,
            time: fmtWhen(it.ts),
            size: sizeStr,
          })}
        </span>
      </span>
      {showActs ? (
        <span style={{ display: "inline-flex", gap: 6, flex: "none" }}>
          {isTxt ? (
            <ActionChip
              label={t("inbox.copyAction")}
              fg="var(--accent-ink)"
              onClick={(e) => {
                e.stopPropagation();
                onCopy(it);
              }}
            />
          ) : (
            <>
              <ActionChip
                label={t("inbox.openAction")}
                fg="var(--accent-ink)"
                onClick={(e) => {
                  e.stopPropagation();
                  onOpen(it);
                }}
              />
              <ActionChip
                label={t("inbox.showAction")}
                fg="var(--muted2)"
                onClick={(e) => {
                  e.stopPropagation();
                  onReveal(it);
                }}
              />
            </>
          )}
          <ActionChip
            label={t("common.forward")}
            fg="var(--muted2)"
            onClick={(e) => onOpenMenu(e, it.id, "fwd")}
          />
          <ActionChip
            label="⋯"
            fg="var(--muted2)"
            onClick={(e) => onOpenMenu(e, it.id, "full")}
          />
        </span>
      ) : (
        <>
          {isTxt && copied && (
            <span
              style={{
                fontSize: 10,
                fontWeight: 600,
                color: "var(--success)",
                background: "var(--success-soft)",
                borderRadius: 99,
                padding: "2px 8px",
                flex: "none",
              }}
            >
              {t("inbox.clipboarded")}
            </span>
          )}
          {/* biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown */}
          <span
            role="button"
            tabIndex={0}
            onClick={(e) => {
              e.stopPropagation();
              if (isTxt) onCopy(it);
              else onReveal(it);
            }}
            onKeyDown={(e) => {
              if (e.key !== "Enter" && e.key !== " ") return;
              if (e.key === " ") e.preventDefault();
              e.stopPropagation();
              if (isTxt) onCopy(it);
              else onReveal(it);
            }}
            style={{
              fontSize: 11,
              color: "var(--accent-ink)",
              cursor: "pointer",
              flex: "none",
            }}
          >
            {isTxt ? t("inbox.copyAction") : t("inbox.showPos")}
          </span>
        </>
      )}
    </div>
  );
});

export default function InboxPage() {
  const { t } = useTranslation();
  const items = useInbox((s) => s.items);
  const devices = useData((s) => s.devices);
  const downloadDir = useData((s) => s.downloadDir);
  const themeMode = usePrefs((s) => s.themeMode);
  const sysDark = useSysDark((s) => s.dark);
  const dark = resolvedTheme(themeMode, sysDark) === "dark";

  const [filter, setFilter] = useState<IbFilter>("all");
  const [sel, setSel] = useState<string[]>([]);
  const [menu, setMenu] = useState<MenuState | null>(null);
  // Text items whose content was copied to the clipboard in this session.
  const [copiedIds, setCopiedIds] = useState<string[]>([]);

  const unread = useInbox((s) => s.unread);
  useEffect(() => {
    // Clear whenever items arrive while the page is open, not just on mount.
    if (unread) useInbox.getState().clearUnread();
  }, [unread]);

  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  const selMode = sel.length > 0;
  // Stable identity (deps drive re-render) so the memoized InboxRow can bail
  // out when unrelated page state (menu/selection) changes.
  const toggleSel = useCallback(
    (id: string) =>
      setSel((cur) =>
        cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id],
      ),
    [],
  );

  const catOk = useCallback(
    (it: InboxItem): boolean =>
      filter === "all"
        ? true
        : filter === "txt"
          ? it.kind === "txt"
          : filter === "doc"
            ? ["doc", "arc", "aud", "oth"].includes(it.kind)
            : it.kind === filter,
    [filter],
  );

  // Keyed on t as well: the group labels are i18n strings that must recompute
  // on a language switch.
  const groups = useMemo(
    () =>
      (
        [
          ["today", t("inbox.today")],
          ["yday", t("inbox.yday")],
          ["earlier", t("inbox.earlier")],
        ] as const
      )
        .map(([k, label]) => ({
          k,
          label,
          rows: items.filter((it) => whenGroup(it.ts) === k && catOk(it)),
        }))
        .filter((g) => g.rows.length > 0),
    [items, catOk, t],
  );

  /* ── row / menu actions ─────────────────────────────────────────────── */

  /** Item display name: multi-file items read as "name 等 N 个文件" (N = total). */
  const displayName = useCallback(
    (it: InboxItem) =>
      it.count > 1
        ? t("transfers.filesMore", { name: it.name, n: it.count })
        : it.name,
    [t],
  );

  const doCopy = useCallback(
    (it: InboxItem) => {
      copyText(it.text ?? "");
      setCopiedIds((cur) => (cur.includes(it.id) ? cur : [...cur, it.id]));
      showToast(t("inbox.copiedText"));
    },
    [t],
  );

  const doOpen = useCallback(
    (it: InboxItem) => {
      const p = it.paths?.[0];
      if (!p) {
        showToast(t("inbox.noPathToast"));
        return;
      }
      openFile(p)
        .then(() => {
          // openFile already toasts milestoneNote in browser mode — don't overwrite it.
          if (isTauri)
            showToast(t("inbox.openToast", { name: displayName(it) }));
        })
        // Blame the RECORD only when the file is genuinely gone. Anything else is
        // an open failure and must say so — this used to swallow every error and
        // pin it on a "stale record", which hid a real bug for a long time.
        .catch((e) =>
          showToast(isNotFound(e) ? t("inbox.noPathToast") : errText(e)),
        );
    },
    [t, displayName],
  );

  const doReveal = useCallback(
    (it: InboxItem) => {
      const p = it.paths?.[0];
      if (!p) {
        // Old-session record with no stored path (received by a prior build /
        // before restart): open the download folder so the user can still find
        // the file by name, instead of a dead-end toast.
        if (downloadDir) {
          showToast(t("inbox.noPathToast"));
          void openDir(downloadDir);
        } else {
          showToast(t("inbox.noPathToast"));
        }
        return;
      }
      void revealFile(p);
    },
    [t, downloadDir],
  );

  const rmItem = (it: InboxItem) => {
    useInbox.getState().remove([it.id]);
    setSel((cur) => cur.filter((x) => x !== it.id));
    setMenu(null);
    showToast(
      t(it.kind === "txt" ? "inbox.removedTextToast" : "inbox.removedToast"),
    );
  };

  const rmBatch = (ids: string[]) => {
    useInbox.getState().remove(ids);
    setSel([]);
    setMenu(null);
    showToast(t("inbox.removedBatchToast", { n: ids.length }));
  };

  const fwdItem = (it: InboxItem, d: DiscoveredDevice) => {
    setMenu(null);
    if (it.kind === "txt") {
      // Forward over the real text channel (M7.3), recording a sent-text entry
      // in the transfer history. alsoClipboard=true mirrors QuickTextModal's
      // default: it's a request the receiver's own clip-share consent still
      // gates. Browser demo mode resolves as a no-op, so the success toast fires
      // there too — no crash.
      sendTextTracked(d.deviceId, d.name, it.text ?? "", true)
        .then(() => showToast(t("inbox.fwdTextToast", { device: d.name })))
        .catch((e) => showToast(errText(e)));
      setSel([]);
      return;
    }
    if (!it.paths?.length) {
      showToast(t("inbox.noPathToast"));
      return;
    }
    if (sendToDevice(d, it.paths.map(sendFileFromPath)))
      showToast(t("inbox.fwdToast", { name: displayName(it), device: d.name }));
    setSel([]);
  };

  const fwdBatch = async (ids: string[], d: DiscoveredDevice) => {
    setMenu(null);
    const selected = items.filter((i) => ids.includes(i.id));
    const txts = selected.filter((i) => i.kind === "txt");
    const files = selected.filter((i) => i.kind !== "txt" && i.paths?.length);
    const paths = files.flatMap((i) => i.paths ?? []);
    if (!paths.length && !txts.length) {
      showToast(t("inbox.noPathToast"));
      return;
    }
    setSel([]);
    // Files ride the transfer path: queued here, and any later failure surfaces
    // through transfer_error (which now speaks up). Texts ride the text channel
    // and resolve — or REJECT — right here: the peer can be offline, too old to
    // receive text, or rate-limited.
    //
    // Those rejections used to land in a bare `.catch(() => {})` underneath an
    // unconditional 「已转发 N 项」. A forward that failed on every single item
    // still reported success, to the user AND to the log. Report what happened.
    const queued = paths.length
      ? sendToDevice(d, paths.map(sendFileFromPath))
      : true;
    const sent = await Promise.allSettled(
      txts.map((it) =>
        sendTextTracked(d.deviceId, d.name, it.text ?? "", true),
      ),
    );
    const rejected = sent.filter((r) => r.status === "rejected");
    const total = files.length + txts.length;
    const ok = total - rejected.length - (queued ? 0 : files.length);
    if (ok === total) {
      showToast(t("inbox.fwdBatchToast", { n: total, device: d.name }));
      return;
    }
    const first = rejected[0] as PromiseRejectedResult | undefined;
    showToast(
      t("inbox.fwdBatchPartial", {
        ok,
        n: total,
        err: first ? errText(first.reason) : t("errors.generic"),
      }),
      undefined,
      7000,
    );
  };

  const openMenu = useCallback(
    (e: ReactMouseEvent, id: string, mode: "fwd" | "full") => {
      e.preventDefault();
      e.stopPropagation();
      const mh = mode === "fwd" ? 20 + Math.max(devices.length, 1) * 29 : 252;
      const x = Math.max(8, Math.min(e.clientX, window.innerWidth - 202));
      const y = Math.max(8, Math.min(e.clientY, window.innerHeight - mh - 10));
      setMenu({ id, mode, x: Math.round(x), y: Math.round(y), sub: false });
    },
    [devices.length],
  );

  const openBarFwd = (e: ReactMouseEvent) => {
    const first = sel[0];
    if (!first) return;
    const mh = 20 + Math.max(devices.length, 1) * 29;
    const x = Math.max(8, Math.min(e.clientX - 95, window.innerWidth - 202));
    const y = Math.max(8, window.innerHeight - mh - 66);
    setMenu({
      id: first,
      mode: "fwd",
      x: Math.round(x),
      y: Math.round(y),
      sub: false,
    });
  };

  const setSub = (sub: boolean) =>
    setMenu((m) => (m && m.sub !== sub ? { ...m, sub } : m));

  /* ── context-menu entries ───────────────────────────────────────────── */

  const mItem = menu ? items.find((i) => i.id === menu.id) : undefined;
  const batchIds =
    menu && mItem && sel.includes(menu.id) && sel.length > 1 ? sel : null;

  let entries: MenuEntry[] = [];
  if (menu && mItem) {
    const devEntries: MenuEntry[] = devices.length
      ? devices.map((d) => ({
          label: d.name,
          dot: "var(--success)",
          onClick: () => {
            if (batchIds) void fwdBatch(batchIds, d);
            else fwdItem(mItem, d);
          },
        }))
      : [{ label: t("trusted.noDevices"), fg: "var(--muted)" }];
    if (menu.mode === "fwd") {
      entries = devEntries;
    } else if (batchIds) {
      entries = [
        {
          label: t("inbox.forwardBatch", { n: batchIds.length }),
          hasSub: true,
          onClick: () => setSub(true),
        },
        { sep: true },
        {
          label: t("inbox.removeBatch", { n: batchIds.length }),
          fg: "var(--muted2)",
          note: t("inbox.menuRemoveNote"),
          onClick: () => rmBatch(batchIds),
        },
      ];
    } else if (mItem.kind === "txt") {
      entries = [
        {
          label: t("inbox.menuCopy"),
          onClick: () => {
            setMenu(null);
            doCopy(mItem);
          },
        },
        {
          label: t("inbox.menuForward"),
          hasSub: true,
          onClick: () => setSub(true),
        },
        { sep: true },
        {
          label: t("inbox.menuRemove"),
          fg: "var(--muted2)",
          onClick: () => rmItem(mItem),
        },
      ];
    } else {
      entries = [
        {
          label: t("inbox.menuOpen"),
          onClick: () => {
            setMenu(null);
            doOpen(mItem);
          },
        },
        {
          label: t("inbox.menuShow"),
          onClick: () => {
            setMenu(null);
            doReveal(mItem);
          },
        },
        {
          label: t("inbox.menuForward"),
          hasSub: true,
          onClick: () => setSub(true),
        },
        { sep: true },
        {
          label: t("inbox.menuRemove"),
          fg: "var(--muted2)",
          note: t("inbox.menuRemoveNote"),
          onClick: () => rmItem(mItem),
        },
      ];
    }
  }

  // Submenu position: flip to the left near the right window edge.
  const subX = menu
    ? menu.x + 190 + 184 > window.innerWidth
      ? menu.x - 178
      : menu.x + 188
    : 0;
  const subOff = batchIds ? 8 : mItem?.kind === "txt" ? 36 : 66;
  const subY = menu
    ? Math.max(
        8,
        Math.min(
          menu.y + subOff,
          window.innerHeight - (Math.max(devices.length, 1) * 29 + 24),
        ),
      )
    : 0;

  /* ── batch bar values (toast-bg pill uses theme-fixed contrast colors) ─ */

  const barFwdFg = dark ? "#e8f4f4" : "#0d1319";
  const barBtnBorder = dark ? "rgba(16,21,29,.3)" : "rgba(255,255,255,.28)";
  const barDelFg = dark ? "#b06a5e" : "#e8a79c";
  const selBytes = items.reduce(
    (a, i) => (sel.includes(i.id) ? a + i.sizeBytes : a),
    0,
  );
  const barBtn: CSSProperties = {
    height: 26,
    display: "inline-flex",
    alignItems: "center",
    padding: "0 11px",
    borderRadius: 7,
    fontSize: 10.5,
    fontWeight: 600,
    cursor: "pointer",
    whiteSpace: "nowrap",
    boxSizing: "border-box",
  };
  const barShow = () => {
    const withPath = items.find((i) => sel.includes(i.id) && i.paths?.length);
    const p = withPath?.paths?.[0];
    if (p) void revealFile(p);
    else showToast(t("inbox.noPathToast"));
  };
  const barDelete = () => {
    const n = sel.length;
    useInbox.getState().remove(sel);
    setSel([]);
    showToast(t("inbox.removedBatchToast", { n }));
  };

  /* ── render ─────────────────────────────────────────────────────────── */

  return (
    <div
      className="scroll-y"
      style={{ flex: 1, animation: "lbFade .18s ease" }}
    >
      <div
        style={{ maxWidth: 760, margin: "0 auto", padding: "22px 24px 30px" }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <Segmented<IbFilter>
            options={[
              { key: "all", label: t("inbox.filterAll") },
              { key: "img", label: t("inbox.filterImg") },
              { key: "vid", label: t("inbox.filterVid") },
              { key: "doc", label: t("inbox.filterDoc") },
              { key: "txt", label: t("inbox.filterTxt") },
            ]}
            value={filter}
            onChange={setFilter}
          />
          <div style={{ flex: 1 }} />
          {/* biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown */}
          <span
            role="button"
            tabIndex={0}
            onClick={() => {
              if (downloadDir) void openFile(downloadDir);
            }}
            onKeyDown={(e) => {
              if (e.key !== "Enter" && e.key !== " ") return;
              if (e.key === " ") e.preventDefault();
              if (downloadDir) void openFile(downloadDir);
            }}
            title={downloadDir}
            style={{
              fontFamily: "var(--mono)",
              fontSize: 11,
              color: "var(--accent-ink)",
              cursor: "pointer",
              whiteSpace: "nowrap",
              minWidth: 0,
              display: "inline-flex",
              alignItems: "center",
              gap: 4,
            }}
          >
            <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
              {downloadDir}
            </span>
            <span style={{ flex: "none" }}>↗</span>
          </span>
        </div>

        {groups.map((g) => (
          <div key={g.k}>
            <div
              style={{
                fontSize: 10.5,
                fontWeight: 600,
                letterSpacing: ".07em",
                color: "var(--muted)",
                padding: "16px 2px 6px",
              }}
            >
              {g.label}
            </div>
            <div className="card">
              {g.rows.map((it) => (
                <InboxRow
                  key={it.id}
                  it={it}
                  name={displayName(it)}
                  checked={sel.includes(it.id)}
                  selMode={selMode}
                  menuOpen={menu?.id === it.id}
                  copied={copiedIds.includes(it.id)}
                  t={t}
                  onToggleSel={toggleSel}
                  onOpenMenu={openMenu}
                  onCopy={doCopy}
                  onOpen={doOpen}
                  onReveal={doReveal}
                />
              ))}
            </div>
          </div>
        ))}

        {groups.length === 0 && (
          <div
            className="card"
            style={{
              padding: 30,
              textAlign: "center",
              fontSize: 12,
              color: "var(--muted)",
              marginTop: 16,
            }}
          >
            {t("inbox.empty")}
          </div>
        )}
      </div>

      {/* ── context menu + forward submenu ─────────────────────────────── */}
      {menu && (
        <>
          {/* biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc */}
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
          <div
            style={{ position: "fixed", inset: 0, zIndex: 60 }}
            onClick={() => setMenu(null)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu(null);
            }}
          />
          <div style={{ ...menuPanelStyle, left: menu.x, top: menu.y }}>
            {entries.map((en) =>
              en.sep ? (
                <div
                  key="menu-sep"
                  style={{
                    height: 1,
                    background: "var(--border)",
                    margin: "4px 6px",
                  }}
                />
              ) : (
                // biome-ignore lint/a11y/useSemanticElements: keep the styled div (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown
                <div
                  key={en.label}
                  role="button"
                  tabIndex={0}
                  onClick={en.onClick}
                  onKeyDown={(e) => {
                    if (e.key !== "Enter" && e.key !== " ") return;
                    if (e.key === " ") e.preventDefault();
                    en.onClick?.();
                  }}
                  onMouseEnter={(e) => {
                    setSub(!!en.hasSub);
                    e.currentTarget.style.background = "var(--hover)";
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.background =
                      en.hasSub && menu.sub ? "var(--hover)" : "transparent";
                  }}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                    padding: "6px 10px",
                    borderRadius: 6,
                    cursor: "pointer",
                    background:
                      en.hasSub && menu.sub ? "var(--hover)" : "transparent",
                  }}
                >
                  {en.dot && (
                    <span
                      style={{
                        width: 6,
                        height: 6,
                        borderRadius: "50%",
                        background: en.dot,
                        flex: "none",
                      }}
                    />
                  )}
                  <span style={{ flex: 1, minWidth: 0 }}>
                    <span
                      style={{
                        display: "block",
                        fontSize: 11.5,
                        color: en.fg ?? "var(--ink)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {en.label}
                    </span>
                    {en.note && (
                      <span
                        style={{
                          display: "block",
                          fontSize: 9.5,
                          color: "var(--muted)",
                          marginTop: 1,
                        }}
                      >
                        {en.note}
                      </span>
                    )}
                  </span>
                  {en.hasSub && (
                    <span
                      style={{
                        fontSize: 9,
                        color: "var(--muted)",
                        flex: "none",
                      }}
                    >
                      ▸
                    </span>
                  )}
                </div>
              ),
            )}
          </div>
          {menu.sub && menu.mode !== "fwd" && (
            <div
              style={{
                ...menuPanelStyle,
                left: subX,
                top: subY,
                zIndex: 62,
                width: 180,
              }}
            >
              {devices.length ? (
                devices.map((d) => (
                  // biome-ignore lint/a11y/useSemanticElements: keep the styled div (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown
                  <div
                    key={d.deviceId}
                    role="button"
                    tabIndex={0}
                    onClick={() => {
                      if (batchIds) void fwdBatch(batchIds, d);
                      else if (mItem) fwdItem(mItem, d);
                    }}
                    onKeyDown={(e) => {
                      if (e.key !== "Enter" && e.key !== " ") return;
                      if (e.key === " ") e.preventDefault();
                      if (batchIds) void fwdBatch(batchIds, d);
                      else if (mItem) fwdItem(mItem, d);
                    }}
                    onMouseEnter={(e) =>
                      (e.currentTarget.style.background = "var(--hover)")
                    }
                    onMouseLeave={(e) =>
                      (e.currentTarget.style.background = "transparent")
                    }
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 8,
                      padding: "6px 10px",
                      borderRadius: 6,
                      cursor: "pointer",
                    }}
                  >
                    <span
                      style={{
                        width: 6,
                        height: 6,
                        borderRadius: "50%",
                        background: "var(--success)",
                        flex: "none",
                      }}
                    />
                    <span
                      style={{
                        flex: 1,
                        minWidth: 0,
                        fontSize: 11.5,
                        color: "var(--ink)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {d.name}
                    </span>
                  </div>
                ))
              ) : (
                <div
                  style={{
                    padding: "6px 10px",
                    fontSize: 11.5,
                    color: "var(--muted)",
                  }}
                >
                  {t("trusted.noDevices")}
                </div>
              )}
            </div>
          )}
        </>
      )}

      {/* ── batch action bar ───────────────────────────────────────────── */}
      {selMode && (
        <div
          style={{
            position: "fixed",
            left: 214,
            right: 0,
            bottom: 22,
            zIndex: 44,
            display: "flex",
            justifyContent: "center",
            pointerEvents: "none",
          }}
        >
          <div
            style={{
              pointerEvents: "auto",
              display: "flex",
              alignItems: "center",
              gap: 9,
              background: "var(--toast-bg)",
              color: "var(--toast-fg)",
              borderRadius: 12,
              boxShadow: "var(--shadow)",
              padding: "9px 14px",
              boxSizing: "border-box",
              animation: "lbUp .18s ease",
              maxWidth: "calc(100% - 48px)",
            }}
          >
            <span
              style={{ fontSize: 11.5, fontWeight: 600, whiteSpace: "nowrap" }}
            >
              {t("inbox.selected", { n: sel.length })}
            </span>
            {selBytes > 0 && (
              <span
                style={{
                  fontFamily: "var(--mono)",
                  fontSize: 10,
                  opacity: 0.55,
                  whiteSpace: "nowrap",
                }}
              >
                {fmtBytes(selBytes)}
              </span>
            )}
            <span style={{ flex: 1 }} />
            {/* biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown */}
            <span
              role="button"
              tabIndex={0}
              onClick={openBarFwd}
              onKeyDown={(e) => {
                if (e.key !== "Enter" && e.key !== " ") return;
                if (e.key === " ") e.preventDefault();
                const r = e.currentTarget.getBoundingClientRect();
                openBarFwd({
                  clientX: r.left + r.width / 2,
                } as unknown as ReactMouseEvent);
              }}
              onMouseEnter={(e) =>
                (e.currentTarget.style.filter = "brightness(.95)")
              }
              onMouseLeave={(e) => (e.currentTarget.style.filter = "none")}
              style={{
                ...barBtn,
                background: "var(--toast-accent)",
                color: barFwdFg,
              }}
            >
              {t("inbox.barForward")}
            </span>
            {/* biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown */}
            <span
              role="button"
              tabIndex={0}
              onClick={barShow}
              onKeyDown={(e) => {
                if (e.key !== "Enter" && e.key !== " ") return;
                if (e.key === " ") e.preventDefault();
                barShow();
              }}
              onMouseEnter={(e) => (e.currentTarget.style.opacity = ".85")}
              onMouseLeave={(e) => (e.currentTarget.style.opacity = "1")}
              style={{ ...barBtn, border: `1px solid ${barBtnBorder}` }}
            >
              {t("inbox.barShow")}
            </span>
            {/* biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown */}
            <span
              role="button"
              tabIndex={0}
              onClick={barDelete}
              onKeyDown={(e) => {
                if (e.key !== "Enter" && e.key !== " ") return;
                if (e.key === " ") e.preventDefault();
                barDelete();
              }}
              onMouseEnter={(e) => (e.currentTarget.style.opacity = ".85")}
              onMouseLeave={(e) => (e.currentTarget.style.opacity = "1")}
              style={{
                ...barBtn,
                border: `1px solid ${barBtnBorder}`,
                color: barDelFg,
              }}
            >
              {t("inbox.barDelete")}
            </span>
            {/* biome-ignore lint/a11y/useSemanticElements: keep the styled span (a native <button> would restyle it); keyboard-operable via role + tabIndex + onKeyDown */}
            <span
              role="button"
              tabIndex={0}
              onClick={() => setSel([])}
              onKeyDown={(e) => {
                if (e.key !== "Enter" && e.key !== " ") return;
                if (e.key === " ") e.preventDefault();
                setSel([]);
              }}
              onMouseEnter={(e) => (e.currentTarget.style.opacity = "1")}
              onMouseLeave={(e) => (e.currentTarget.style.opacity = ".6")}
              style={{
                fontSize: 13,
                opacity: 0.6,
                cursor: "pointer",
                padding: "0 2px",
                lineHeight: 1,
              }}
            >
              ×
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
