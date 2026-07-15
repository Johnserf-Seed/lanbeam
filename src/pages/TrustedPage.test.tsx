// Component tests for the trust-circle page. The env is non-Tauri, so the
// store's fire-and-forget backend writes (api.setTrusted / api.removeTrusted)
// take their browser-mode no-op branch — we mock them to assert the write is
// issued, but the load-bearing assertions read useTrust state directly. Free
// layout (≤8 devices) is used throughout; each test seeds useData.devices +
// useTrust.records, then renders. The page builds a ResizeObserver in a ref
// callback, which happy-dom doesn't implement, so we stub it per test.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DiscoveredDevice } from "../bridge/api";
import i18n from "../i18n";
import type { TrustRecord } from "../lib/store";
import { shortFp, useData, useTrust } from "../lib/store";
import { act, fireEvent, renderUI, screen } from "../test/render";

// Mock only the two trust writes; keep every other export real so the store
// module (which imports the whole namespace at load) still works.
vi.mock("../bridge/api", async (importActual) => {
  const actual = await importActual<typeof import("../bridge/api")>();
  return {
    ...actual,
    setTrusted: vi.fn(() => Promise.resolve()),
    removeTrusted: vi.fn(() => Promise.resolve()),
    forgetDevice: vi.fn(() => Promise.resolve()),
  };
});

import * as api from "../bridge/api";
import TrustedPage from "./TrustedPage";

// Clean snapshots captured once, to reset the touched stores between tests.
const data0 = { ...useData.getState() };
const trust0 = { ...useTrust.getState() };

const dev = (deviceId: string, name: string): DiscoveredDevice => ({
  deviceId,
  name,
  address: "10.0.0.9",
  port: 51704,
});

const rec = (
  over: Partial<TrustRecord> & { deviceId: string },
): TrustRecord => ({
  name: over.name ?? "Peer",
  trusted: true,
  autoAccept: false,
  addedAt: 1000,
  lastSeen: 1000,
  ...over,
});

function seed(
  devices: DiscoveredDevice[],
  records: Record<string, TrustRecord>,
): void {
  useData.setState({ ...data0, devices });
  useTrust.setState({ ...trust0, records, sel: null });
}

beforeEach(() => {
  // happy-dom has no ResizeObserver; the cell ref callback constructs one.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    },
  );
  vi.mocked(api.setTrusted).mockClear();
  vi.mocked(api.removeTrusted).mockClear();
  useData.setState({ ...data0 });
  useTrust.setState({ ...trust0 });
});

describe("TrustedPage", () => {
  it("renders the seeded trusted peers and the trust-circle chrome", () => {
    seed([dev("d1", "Alice"), dev("d2", "Bob")], {
      d1: rec({ deviceId: "d1", name: "Alice" }),
      d2: rec({ deviceId: "d2", name: "Bob", autoAccept: true }),
    });
    renderUI(<TrustedPage />);

    // Both peers render as chips (Alice also appears in the selected bar).
    expect(screen.getAllByText("Alice").length).toBeGreaterThan(0);
    expect(screen.getByText("Bob")).toBeInTheDocument();
    // Circle labels + free-mode hint.
    expect(screen.getByText(i18n.t("trusted.self"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("trusted.hintFree"))).toBeInTheDocument();
  });

  it("shows the selected peer's short fingerprint in the bar", () => {
    // shortFp("vjx0qm8d") → "VJX0 · QM8D"
    const id = "vjx0qm8d";
    seed([dev(id, "Alice")], { [id]: rec({ deviceId: id, name: "Alice" }) });
    renderUI(<TrustedPage />);

    expect(screen.getByText(shortFp(id))).toBeInTheDocument();
  });

  it("renames the selected peer, updating its trust record", () => {
    seed([dev("d1", "Alice")], { d1: rec({ deviceId: "d1", name: "Alice" }) });
    renderUI(<TrustedPage />);

    fireEvent.click(screen.getByText(i18n.t("common.rename")));
    const input = screen.getByDisplayValue("Alice");
    fireEvent.change(input, { target: { value: "Alice Renamed" } });
    fireEvent.blur(input);

    expect(useTrust.getState().records.d1.name).toBe("Alice Renamed");
    // The renamed trusted record is written through to the backend store.
    expect(api.setTrusted).toHaveBeenCalledWith("d1", "Alice Renamed", false);
  });

  it("toggles auto-accept for the selected trusted peer", () => {
    seed([dev("d1", "Alice")], {
      d1: rec({ deviceId: "d1", name: "Alice", autoAccept: false }),
    });
    const { container } = renderUI(<TrustedPage />);

    const toggle = container.querySelector("button.toggle");
    expect(toggle).not.toBeNull();
    fireEvent.click(toggle as Element);

    expect(useTrust.getState().records.d1.autoAccept).toBe(true);
    expect(api.setTrusted).toHaveBeenCalledWith("d1", "Alice", true);
  });

  it("deletes a remembered offline peer from the page and the backend", () => {
    // carol is a remembered offline record with no live device behind it. With
    // nothing announcing her, deleting really does end her.
    seed([dev("d1", "Alice")], {
      d1: rec({ deviceId: "d1", name: "Alice" }),
      carol: rec({ deviceId: "carol", name: "Carol" }),
    });
    renderUI(<TrustedPage />);
    expect(screen.getByText("Carol")).toBeInTheDocument();

    act(() => {
      useTrust.getState().remove("carol");
    });

    expect(screen.queryByText("Carol")).not.toBeInTheDocument();
    expect(useTrust.getState().records.carol).toBeUndefined();
    // forget_device, NOT remove_trusted: deleting has to clear the manually-added
    // ADDRESS too, or the peer is back on the very next device list. That was the
    // whole bug — 「删除」 dropped the trust row and left the address behind.
    expect(api.forgetDevice).toHaveBeenCalledWith("carol");
  });

  it("deleting also drops the device from the live list, so a manual peer really goes", () => {
    // A peer that is only in the list because someone typed its address is the
    // one kind that CAN be deleted for good — nothing will announce it back. It
    // used to be the one kind you could never delete: with no trust record, the
    // drop zone wasn't even offered, and `remove_trusted` couldn't touch the
    // manual address anyway.
    seed([{ ...dev("manual1", "Typed In"), manual: true }], {});
    renderUI(<TrustedPage />);
    // It is the only device, so it renders both as a chip and in the selection
    // bar — hence getAllByText.
    expect(screen.getAllByText("Typed In").length).toBeGreaterThan(0);

    act(() => {
      useTrust.getState().remove("manual1");
    });

    expect(screen.queryByText("Typed In")).not.toBeInTheDocument();
    expect(api.forgetDevice).toHaveBeenCalledWith("manual1");
  });
});
