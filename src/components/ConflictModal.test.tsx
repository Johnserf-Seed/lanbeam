// Component tests for ConflictModal (M6.5 name-collision resolver). The env is
// non-Tauri, so replyFileRequest is a browser-mode no-op; we mock just that one
// export to assert the single reply each choice folds into. Stores are reset to
// a captured snapshot before each test so cases can't leak into one another.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IncomingRequest } from "../bridge/api";
import i18n from "../i18n";
import { useOverlays, usePrefs, useTransfers, useTrust } from "../lib/store";
import { fireEvent, renderUI, screen } from "../test/render";
import ConflictModal from "./ConflictModal";

// Partial mock: keep everything real (isTauri stays false, store.ts keeps its
// actual api) except replyFileRequest, which we spy on.
vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return { ...actual, replyFileRequest: vi.fn(() => Promise.resolve()) };
});
import * as api from "../bridge/api";

const t = (k: string, o?: Record<string, unknown>) => i18n.t(k, o);

// ── snapshots for reset ────────────────────────────────────────────────────
const overlays0 = { ...useOverlays.getState() };
const transfers0 = { ...useTransfers.getState() };
const trust0 = { ...useTrust.getState() };
const prefs0 = { ...usePrefs.getState() };

const makeRequest = (over?: Partial<IncomingRequest>): IncomingRequest => ({
  sessionId: "sess-1",
  deviceId: "peer-dev",
  sas: "123456",
  totalSize: 2048,
  fileCount: 1,
  files: [{ name: "photo.png", size: 2048 }],
  conflicts: ["photo.png"],
  conflictPolicy: "ask",
  ...over,
});

function seedConflict(
  over?: Partial<IncomingRequest>,
  wantTrust = false,
  peerName = "Nova",
) {
  useOverlays.setState({
    conflict: { request: makeRequest(over), peerName, wantTrust },
  });
}

beforeEach(() => {
  useOverlays.setState(overlays0, true);
  useTransfers.setState(transfers0, true);
  useTrust.setState(trust0, true);
  usePrefs.setState(prefs0, true);
  vi.clearAllMocks();
});

describe("ConflictModal rendering", () => {
  it("renders nothing when no conflict is parked", () => {
    const { container } = renderUI(<ConflictModal />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the conflicting filename and the keep-both / overwrite / skip choices", () => {
    seedConflict();
    renderUI(<ConflictModal />);

    // The filename surfaces in the title and the incoming comparison line.
    expect(screen.getAllByText(/photo\.png/).length).toBeGreaterThan(0);

    // All three resolution options render their labels.
    expect(screen.getByText(t("conflict.keepBoth"))).toBeInTheDocument();
    expect(screen.getByText(t("conflict.overwrite"))).toBeInTheDocument();
    expect(screen.getByText(t("conflict.skip"))).toBeInTheDocument();

    // Footer actions.
    expect(
      screen.getByRole("button", { name: t("conflict.go") }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: t("conflict.cancel") }),
    ).toBeInTheDocument();
  });

  it("summarizes extra colliding files with the multi subtitle", () => {
    seedConflict({
      files: [
        { name: "photo.png", size: 2048 },
        { name: "notes.pdf", size: 100 },
      ],
      conflicts: ["photo.png", "notes.pdf"],
      fileCount: 2,
    });
    renderUI(<ConflictModal />);
    expect(
      screen.getByText(t("conflict.subMulti", { n: 1 })),
    ).toBeInTheDocument();
  });
});

describe("ConflictModal choices", () => {
  it("keep-both (default) accepts with the rename conflict action and closes", () => {
    seedConflict();
    renderUI(<ConflictModal />);

    fireEvent.click(screen.getByRole("button", { name: t("conflict.go") }));

    expect(api.replyFileRequest).toHaveBeenCalledTimes(1);
    expect(api.replyFileRequest).toHaveBeenCalledWith("sess-1", true, "rename");
    expect(useOverlays.getState().conflict).toBeNull();
  });

  it("overwrite accepts with the overwrite conflict action and closes", () => {
    seedConflict();
    renderUI(<ConflictModal />);

    // Selecting the option flips the radio; Continue folds it into the reply.
    fireEvent.click(screen.getByText(t("conflict.overwrite")));
    fireEvent.click(screen.getByRole("button", { name: t("conflict.go") }));

    expect(api.replyFileRequest).toHaveBeenCalledTimes(1);
    expect(api.replyFileRequest).toHaveBeenCalledWith(
      "sess-1",
      true,
      "overwrite",
    );
    expect(useOverlays.getState().conflict).toBeNull();
  });

  it("skip declines the whole transfer (no conflict action) and closes", () => {
    seedConflict();
    renderUI(<ConflictModal />);

    fireEvent.click(screen.getByText(t("conflict.skip")));
    fireEvent.click(screen.getByRole("button", { name: t("conflict.go") }));

    expect(api.replyFileRequest).toHaveBeenCalledTimes(1);
    expect(api.replyFileRequest).toHaveBeenCalledWith("sess-1", false);
    expect(useOverlays.getState().conflict).toBeNull();
  });

  it("cancel receiving declines, grants no trust even when deferred, and closes", () => {
    seedConflict(undefined, /* wantTrust */ true);
    renderUI(<ConflictModal />);

    fireEvent.click(screen.getByRole("button", { name: t("conflict.cancel") }));

    // Single decline reply, no accept, no conflict action.
    expect(api.replyFileRequest).toHaveBeenCalledTimes(1);
    expect(api.replyFileRequest).toHaveBeenCalledWith("sess-1", false);
    // Declining grants nothing — the deferred trust is never applied.
    expect(useTrust.getState().records["peer-dev"]).toBeUndefined();
    expect(useOverlays.getState().conflict).toBeNull();
  });

  it("a positive choice applies the deferred trust for the peer", () => {
    seedConflict(undefined, /* wantTrust */ true);
    renderUI(<ConflictModal />);

    fireEvent.click(screen.getByRole("button", { name: t("conflict.go") }));

    const rec = useTrust.getState().records["peer-dev"];
    expect(rec?.trusted).toBe(true);
    expect(api.replyFileRequest).toHaveBeenCalledWith("sess-1", true, "rename");
  });
});
