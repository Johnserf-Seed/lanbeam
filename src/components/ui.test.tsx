// Smoke tests for the shared UI primitives — also the proof that the vitest
// infrastructure (happy-dom + testing-library + jest-dom) is wired correctly.
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { ExtChip, Segmented, StatusDot, Toggle } from "./ui";

describe("ExtChip", () => {
  it("renders the extension text for a normal file", () => {
    render(<ExtChip ext="PDF" />);
    expect(screen.getByText("PDF")).toBeInTheDocument();
  });

  it("renders the text-lines glyph (svg), not the ext, for quick-text", () => {
    const { container } = render(<ExtChip ext="txt" isTxt />);
    expect(container.querySelector("svg")).not.toBeNull();
    expect(screen.queryByText("txt")).toBeNull();
  });
});

describe("Toggle", () => {
  it("reflects on/off state via class and fires onClick", async () => {
    const onClick = vi.fn();
    const { container } = render(<Toggle on onClick={onClick} />);
    const btn = container.querySelector("button");
    expect(btn?.className).toContain("on");
    await userEvent.click(btn as HTMLButtonElement);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

describe("Segmented", () => {
  it("marks the active option and reports selection", async () => {
    const onChange = vi.fn();
    render(
      <Segmented
        options={[
          { key: "a", label: "Alpha" },
          { key: "b", label: "Beta" },
        ]}
        value="a"
        onChange={onChange}
      />,
    );
    expect(screen.getByText("Alpha").className).toContain("active");
    await userEvent.click(screen.getByText("Beta"));
    expect(onChange).toHaveBeenCalledWith("b");
  });
});

describe("StatusDot", () => {
  it("shows a filled dot when online and a hollow one when away", () => {
    const { container: on } = render(<StatusDot online />);
    const { container: off } = render(<StatusDot online={false} />);
    const onDot = on.querySelector("span") as HTMLElement;
    const offDot = off.querySelector("span") as HTMLElement;
    expect(onDot.style.background).not.toBe("transparent");
    expect(offDot.style.background).toBe("transparent");
  });
});
