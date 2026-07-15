// Component tests for IncomingStack — the accept/decline/conflict logic
// (recently hardened). The env is non-Tauri, so the real bridge stubs resolve
// as no-ops; we partial-mock ../bridge/api to keep every real export (isTauri
// stays false so the stores stay in browser mode) and only spy on
// replyFileRequest so we can assert the accept/decline reply. State is driven
// through the zustand stores and asserted via getState().
import { act } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IncomingRequest } from "../bridge/api";
import i18n from "../i18n";
import { useOverlays, useTransfers, useTrust } from "../lib/store";
import { fireEvent, renderUI, screen } from "../test/render";
import IncomingStack from "./IncomingStack";

vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return { ...actual, replyFileRequest: vi.fn(() => Promise.resolve()) };
});

import * as api from "../bridge/api";

// Clean-slate snapshots captured once at module load.
const transfers0 = { ...useTransfers.getState() };
const overlays0 = { ...useOverlays.getState() };
const trust0 = { ...useTrust.getState() };

const req = (over: Partial<IncomingRequest> = {}): IncomingRequest => ({
  sessionId: "rx1",
  deviceId: "peerX",
  sas: "123456",
  totalSize: 2048,
  fileCount: 2,
  files: [
    { name: "a.txt", size: 1024 },
    { name: "b.txt", size: 1024 },
  ],
  senderName: "Alice",
  ...over,
});

beforeEach(() => {
  useTransfers.setState({ ...transfers0, incomings: [] }, true);
  useOverlays.setState({ ...overlays0, conflict: null }, true);
  useTrust.setState({ ...trust0, records: {} }, true);
  vi.mocked(api.replyFileRequest).mockClear();
});

const t = (key: string, vars?: Record<string, unknown>) => i18n.t(key, vars);

describe("IncomingStack rendering", () => {
  it("renders nothing when there are no incoming requests", () => {
    const { container } = renderUI(<IncomingStack />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the peer name and the file summary of the front request", () => {
    useTransfers.setState({ incomings: [req()] });
    renderUI(<IncomingStack />);
    // senderName wins as the display name
    expect(screen.getByText("Alice")).toBeInTheDocument();
    // first file surfaces as a chip; the rest fold into a "+n" label
    expect(screen.getByText("a.txt")).toBeInTheDocument();
    expect(
      screen.getByText(t("incoming.moreLabel", { n: 1 })),
    ).toBeInTheDocument();
    // both action buttons render
    expect(
      screen.getByRole("button", { name: t("incoming.accept") }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: t("incoming.decline") }),
    ).toBeInTheDocument();
  });
});

describe("IncomingStack accept (no conflicts)", () => {
  it("replies true and removes the card after the 240ms leave timer", () => {
    vi.useFakeTimers();
    try {
      useTransfers.setState({ incomings: [req({ sessionId: "s-accept" })] });
      renderUI(<IncomingStack />);

      fireEvent.click(
        screen.getByRole("button", { name: t("incoming.accept") }),
      );

      // Nothing fires until the leave animation timer elapses.
      expect(api.replyFileRequest).not.toHaveBeenCalled();
      expect(useTransfers.getState().incomings).toHaveLength(1);

      act(() => {
        vi.advanceTimersByTime(240);
      });

      expect(api.replyFileRequest).toHaveBeenCalledWith("s-accept", true);
      // card removed and its receive meta staged
      expect(useTransfers.getState().incomings).toHaveLength(0);
      expect(useTransfers.getState().pendingRecv["s-accept"]).toBeDefined();
      // no trust granted without the checkbox
      expect(useTrust.getState().records.peerX).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("IncomingStack accept with trust", () => {
  it("grants trust + auto-accept when the checkbox is ticked", () => {
    vi.useFakeTimers();
    try {
      useTransfers.setState({
        incomings: [req({ sessionId: "s-trust", deviceId: "peerT" })],
      });
      renderUI(<IncomingStack />);

      const checkbox = screen.getByRole("checkbox");
      expect((checkbox as HTMLInputElement).checked).toBe(false);
      fireEvent.click(checkbox);
      expect((checkbox as HTMLInputElement).checked).toBe(true);

      fireEvent.click(
        screen.getByRole("button", { name: t("incoming.accept") }),
      );
      act(() => {
        vi.advanceTimersByTime(240);
      });

      expect(api.replyFileRequest).toHaveBeenCalledWith("s-trust", true);
      const rec = useTrust.getState().records.peerT;
      expect(rec).toBeDefined();
      expect(rec.trusted).toBe(true);
      // Trusting from the prompt enables auto-accept too (setTrust defaults it
      // on for a fresh trust), so future transfers from this device don't nag.
      expect(rec.autoAccept).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("IncomingStack conflict under the ask policy", () => {
  it("defers to the ConflictModal instead of replying directly", () => {
    vi.useFakeTimers();
    try {
      const r = req({
        sessionId: "s-conflict",
        conflicts: ["a.txt"],
        conflictPolicy: "ask",
      });
      useTransfers.setState({ incomings: [r] });
      renderUI(<IncomingStack />);

      fireEvent.click(
        screen.getByRole("button", { name: t("incoming.accept") }),
      );
      act(() => {
        vi.advanceTimersByTime(240);
      });

      // the reply is deferred to the modal — no direct reply here
      expect(api.replyFileRequest).not.toHaveBeenCalled();
      // card removed and the conflict handed to the overlay
      expect(useTransfers.getState().incomings).toHaveLength(0);
      const conflict = useOverlays.getState().conflict;
      expect(conflict?.request.sessionId).toBe("s-conflict");
      expect(conflict?.peerName).toBe("Alice");
      expect(conflict?.wantTrust).toBe(false);
      // trust is deferred to the modal too — nothing granted yet
      expect(useTrust.getState().records.peerX).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("IncomingStack decline", () => {
  it("replies false and removes the card after the leave timer", () => {
    vi.useFakeTimers();
    try {
      useTransfers.setState({ incomings: [req({ sessionId: "s-decline" })] });
      renderUI(<IncomingStack />);

      fireEvent.click(
        screen.getByRole("button", { name: t("incoming.decline") }),
      );
      expect(api.replyFileRequest).not.toHaveBeenCalled();

      act(() => {
        vi.advanceTimersByTime(240);
      });

      expect(api.replyFileRequest).toHaveBeenCalledWith("s-decline", false);
      expect(useTransfers.getState().incomings).toHaveLength(0);
      // decline never stages receive meta
      expect(useTransfers.getState().pendingRecv["s-decline"]).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("IncomingStack front-card swap resets the trust checkbox", () => {
  it("clears the ticked checkbox when the front request changes", () => {
    useTransfers.setState({
      incomings: [
        req({ sessionId: "front", deviceId: "peerA" }),
        req({ sessionId: "next", deviceId: "peerB" }),
      ],
    });
    renderUI(<IncomingStack />);

    const checkbox = screen.getByRole("checkbox");
    fireEvent.click(checkbox);
    expect((checkbox as HTMLInputElement).checked).toBe(true);

    // The front card is swapped out from under us (e.g. its session errored and
    // AppShell removed it, promoting the queued request).
    act(() => {
      useTransfers.getState().removeIncoming("front");
    });

    // frontSessionId changed → the effect resets trustPending.
    expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(
      false,
    );
  });
});
