// The app-native right-click menu that replaces the WebView's browser menu.
// Covers: the browser menu is always suppressed; a custom Cut/Copy/Paste menu
// opens on text inputs only; it dismisses on Esc / outside click.
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import InputContextMenu from "./InputContextMenu";
import { act, fireEvent, renderUI, screen } from "../test/render";

/** Right-click `el` and flush the resulting React state update. Returns the
 *  event so callers can assert `defaultPrevented` (the browser menu is killed). */
const rightClick = (el: Element): MouseEvent => {
  const ev = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    clientX: 20,
    clientY: 20,
  });
  act(() => {
    el.dispatchEvent(ev);
  });
  return ev;
};

describe("InputContextMenu", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("suppresses the browser menu and opens a custom menu on a text input", () => {
    const input = document.createElement("input");
    input.value = "hello world";
    document.body.appendChild(input);
    renderUI(<InputContextMenu />);

    const ev = rightClick(input);

    // The default (browser) context menu is always cancelled.
    expect(ev.defaultPrevented).toBe(true);
    expect(screen.getByText(i18n.t("menu.cut"))).toBeTruthy();
    expect(screen.getByText(i18n.t("menu.copy"))).toBeTruthy();
    expect(screen.getByText(i18n.t("menu.paste"))).toBeTruthy();
    expect(screen.getByText(i18n.t("menu.selectAll"))).toBeTruthy();
  });

  it("opens on a textarea too", () => {
    const ta = document.createElement("textarea");
    document.body.appendChild(ta);
    renderUI(<InputContextMenu />);
    rightClick(ta);
    expect(screen.getByText(i18n.t("menu.paste"))).toBeTruthy();
  });

  it("suppresses the menu on a non-editable element without opening ours", () => {
    const div = document.createElement("div");
    document.body.appendChild(div);
    renderUI(<InputContextMenu />);

    const ev = rightClick(div);

    expect(ev.defaultPrevented).toBe(true); // browser menu still killed
    expect(screen.queryByText(i18n.t("menu.cut"))).toBeNull(); // but no custom menu
  });

  it("skips non-text inputs (checkbox has nothing to cut/paste)", () => {
    const cb = document.createElement("input");
    cb.type = "checkbox";
    document.body.appendChild(cb);
    renderUI(<InputContextMenu />);
    rightClick(cb);
    expect(screen.queryByText(i18n.t("menu.copy"))).toBeNull();
  });

  it("disables Cut/Copy when nothing is selected", () => {
    const input = document.createElement("input");
    input.value = "abc";
    document.body.appendChild(input);
    renderUI(<InputContextMenu />);
    rightClick(input);
    // No selection → Cut & Copy are present but disabled; Paste stays enabled.
    expect(
      screen.getByText(i18n.t("menu.cut")).closest("button"),
    ).toBeDisabled();
    expect(
      screen.getByText(i18n.t("menu.copy")).closest("button"),
    ).toBeDisabled();
    expect(
      screen.getByText(i18n.t("menu.paste")).closest("button"),
    ).not.toBeDisabled();
  });

  it("enables Cut/Copy when text is selected under the cursor", () => {
    const input = document.createElement("input");
    input.value = "hello";
    document.body.appendChild(input);
    input.focus();
    input.setSelectionRange(0, 5); // whole value selected
    renderUI(<InputContextMenu />);
    rightClick(input);
    expect(
      screen.getByText(i18n.t("menu.cut")).closest("button"),
    ).not.toBeDisabled();
    expect(
      screen.getByText(i18n.t("menu.copy")).closest("button"),
    ).not.toBeDisabled();
  });

  it("dismisses on Escape", () => {
    const input = document.createElement("input");
    document.body.appendChild(input);
    renderUI(<InputContextMenu />);
    rightClick(input);
    expect(screen.getByText(i18n.t("menu.copy"))).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByText(i18n.t("menu.copy"))).toBeNull();
  });
});
