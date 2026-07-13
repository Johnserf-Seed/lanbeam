import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { copyText, readClipboardText } from "../lib/sendops";

/** Replaces the WebView's default (browser) right-click menu app-wide. Every
 *  `contextmenu` is prevented — so the app never shows the browser's
 *  reload/back/inspect menu — and a small, app-styled Cut/Copy/Paste/Select-All
 *  menu is offered on text inputs & textareas instead. Non-editable areas simply
 *  get no menu (a native-feeling desktop app, not a web page). */

type EditableEl = HTMLInputElement | HTMLTextAreaElement;

/** Input types whose right-click deserves an editing menu (text-like only — a
 *  checkbox / range / color input has nothing to cut or paste). */
const TEXT_TYPES = new Set([
  "text",
  "search",
  "url",
  "email",
  "tel",
  "password",
  "number",
  "",
]);

function editableTarget(t: EventTarget | null): EditableEl | null {
  if (t instanceof HTMLTextAreaElement) return t;
  if (t instanceof HTMLInputElement && TEXT_TYPES.has(t.type)) return t;
  return null;
}

/** Set a value on a controlled input the way React can see: go through the
 *  prototype's native value setter, then dispatch a bubbling `input` event so
 *  React's change tracker fires onChange. A plain `el.value = x` is invisible to
 *  React and would be reverted on the next render. */
function setNativeValue(el: EditableEl, value: string) {
  const proto =
    el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

export default function InputContextMenu() {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    el: EditableEl;
  } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // One global handler: kill the browser menu everywhere, open ours on inputs.
  useEffect(() => {
    const onCtx = (e: MouseEvent) => {
      e.preventDefault();
      const el = editableTarget(e.target);
      setMenu(el && !el.disabled ? { x: e.clientX, y: e.clientY, el } : null);
    };
    document.addEventListener("contextmenu", onCtx);
    return () => document.removeEventListener("contextmenu", onCtx);
  }, []);

  // While open: dismiss on an outside click, Esc, scroll, or window blur.
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [menu]);

  if (!menu) return null;

  const el = menu.el;
  const start = el.selectionStart ?? 0;
  const end = el.selectionEnd ?? 0;
  const hasSel = start !== end;
  const readOnly = el.readOnly;
  const selected = () => el.value.slice(start, end);

  const copy = () => {
    if (hasSel) copyText(selected());
  };
  const cut = () => {
    if (!hasSel || readOnly) return;
    copyText(selected());
    setNativeValue(el, el.value.slice(0, start) + el.value.slice(end));
    el.focus();
    el.setSelectionRange(start, start);
  };
  const paste = async () => {
    if (readOnly) return;
    // Reads via the Tauri clipboard plugin in-app (no WebView permission prompt).
    const text = await readClipboardText();
    if (!text) return;
    setNativeValue(el, el.value.slice(0, start) + text + el.value.slice(end));
    el.focus();
    const pos = start + text.length;
    el.setSelectionRange(pos, pos);
  };
  const selectAll = () => {
    el.focus();
    el.select();
  };

  const items = [
    { label: t("menu.cut"), fn: cut, off: !hasSel || readOnly },
    { label: t("menu.copy"), fn: copy, off: !hasSel },
    { label: t("menu.paste"), fn: paste, off: readOnly },
    { label: t("menu.selectAll"), fn: selectAll, off: el.value.length === 0 },
  ];

  // Keep the menu on-screen near the cursor.
  const W = 168;
  const H = items.length * 30 + 8;
  const x = Math.max(4, Math.min(menu.x, window.innerWidth - W - 4));
  const y = Math.max(4, Math.min(menu.y, window.innerHeight - H - 4));

  return (
    <div
      ref={menuRef}
      style={{
        position: "fixed",
        left: x,
        top: y,
        zIndex: 100,
        minWidth: W,
        background: "var(--panel)",
        border: "1px solid var(--border2)",
        borderRadius: 10,
        boxShadow: "var(--shadow)",
        overflow: "hidden",
        padding: 4,
        fontFamily: "var(--font)",
        animation: "lbFade .12s ease",
      }}
    >
      {items.map((it) => (
        <button
          key={it.label}
          type="button"
          disabled={it.off}
          onClick={() => {
            void it.fn();
            setMenu(null);
          }}
          style={{
            display: "block",
            width: "100%",
            textAlign: "left",
            padding: "6px 12px",
            fontSize: 12,
            border: "none",
            background: "none",
            borderRadius: 6,
            color: it.off ? "var(--muted2)" : "var(--ink2)",
            cursor: it.off ? "default" : "pointer",
            opacity: it.off ? 0.5 : 1,
          }}
          onMouseEnter={(e) => {
            if (!it.off) e.currentTarget.style.background = "var(--hover)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "none";
          }}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}
