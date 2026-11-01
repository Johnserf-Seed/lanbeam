import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "../test/render";
import { useEscape } from "./useEscape";

function Dialog({
  active,
  onEscape,
}: {
  active: boolean;
  onEscape: () => void;
}) {
  useEscape(active, onEscape);
  return null;
}

describe("useEscape", () => {
  it("closes the top dialog only — never the one underneath too", () => {
    const below = vi.fn();
    const above = vi.fn();
    const { rerender } = render(
      <>
        <Dialog active onEscape={below} />
        <Dialog active onEscape={above} />
      </>,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(above).toHaveBeenCalledTimes(1);
    expect(below).not.toHaveBeenCalled();

    // The top one closes; the next Escape reaches the one below.
    rerender(
      <>
        <Dialog active onEscape={below} />
        <Dialog active={false} onEscape={above} />
      </>,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(below).toHaveBeenCalledTimes(1);
    expect(above).toHaveBeenCalledTimes(1);
  });

  it("ignores an input method's Escape, other keys, and inactive dialogs", () => {
    const fn = vi.fn();
    const { rerender } = render(<Dialog active onEscape={fn} />);
    fireEvent.keyDown(window, { key: "Escape", isComposing: true });
    fireEvent.keyDown(window, { key: "Enter" });
    expect(fn).not.toHaveBeenCalled();
    rerender(<Dialog active={false} onEscape={fn} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(fn).not.toHaveBeenCalled();
  });

  it("calls the LATEST handler, not the one from the first render", () => {
    const first = vi.fn();
    const second = vi.fn();
    const { rerender } = render(<Dialog active onEscape={first} />);
    rerender(<Dialog active onEscape={second} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });
});
