import { useEffect, useRef } from "react";

/** The open dialogs' Escape handlers, most recently opened last. Escape closes
 *  only the top one — a dialog opened over another must not take both down. */
const stack: { run: () => void }[] = [];

function onKeyDown(e: KeyboardEvent): void {
  // An Escape an input method is using to cancel its composition, or one a
  // menu / field already handled, is not a request to close the dialog.
  if (e.key !== "Escape" || e.isComposing || e.defaultPrevented) return;
  const top = stack[stack.length - 1];
  if (!top) return;
  e.preventDefault();
  top.run();
}

/** Close a dialog on Escape while `active` — the keyboard twin of clicking its
 *  backdrop, so pass the same handler the backdrop uses. */
export function useEscape(active: boolean, onEscape: () => void): void {
  const latest = useRef(onEscape);
  latest.current = onEscape;
  useEffect(() => {
    if (!active) return;
    const entry = { run: () => latest.current() };
    stack.push(entry);
    if (stack.length === 1) window.addEventListener("keydown", onKeyDown);
    return () => {
      const i = stack.lastIndexOf(entry);
      if (i >= 0) stack.splice(i, 1);
      if (stack.length === 0) window.removeEventListener("keydown", onKeyDown);
    };
  }, [active]);
}
