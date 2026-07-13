// Tests for browser-mode demo seeding. Each test re-imports the module graph
// via vi.resetModules() so the module-level `seeded` flag and the zustand store
// singletons are fresh (no cross-test leakage).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Helper: set the URL query string so maybeSeedDemo's `?demo` check can pass/fail.
function setSearch(search: string): void {
  window.history.replaceState(null, "", `/${search}`);
}

// Load a fresh copy of demo.ts plus the stores it mutates, all sharing one
// freshly-reset module graph so their state is isolated per call.
async function freshModules() {
  vi.resetModules();
  const demo = await import("./demo");
  const store = await import("./store");
  return { ...demo, ...store };
}

beforeEach(() => {
  setSearch("?demo=1");
});

afterEach(() => {
  setSearch("");
  vi.resetModules();
});

describe("maybeSeedDemo", () => {
  it("does nothing when the ?demo flag is absent", async () => {
    setSearch("");
    const { maybeSeedDemo, useTransfers, useTrust, useInbox } =
      await freshModules();
    maybeSeedDemo();
    expect(useTransfers.getState().transfers).toEqual({});
    expect(useTransfers.getState().incomings).toEqual([]);
    expect(useTrust.getState().records).toEqual({});
    expect(useInbox.getState().items).toEqual([]);
  });

  it("seeds transfers, incomings, trust records and inbox on first call", async () => {
    const { maybeSeedDemo, useTransfers, useTrust, useInbox } =
      await freshModules();
    maybeSeedDemo();

    const t = useTransfers.getState();
    // The six demo transfer/text records keyed d1..d4, t1, t2.
    expect(Object.keys(t.transfers).sort()).toEqual([
      "d1",
      "d2",
      "d3",
      "d4",
      "t1",
      "t2",
    ]);
    expect(t.transfers.d1.name).toBe("产品设计稿 v2.zip");
    expect(t.transfers.d1.direction).toBe("send");
    expect(t.transfers.d2.status).toBe("done");
    expect(t.transfers.d4.status).toBe("error");
    expect(t.transfers.t1.kind).toBe("text");

    // One pending incoming request.
    expect(t.incomings).toHaveLength(1);
    expect(t.incomings[0].sas).toBe("483921");
    expect(t.incomings[0].files).toHaveLength(3);

    // Two trust records.
    expect(Object.keys(useTrust.getState().records).sort()).toEqual([
      "demo-mini",
      "demo-nas",
    ]);
    expect(useTrust.getState().records["demo-mini"].trusted).toBe(true);

    // Five inbox items.
    expect(useInbox.getState().items).toHaveLength(5);
    expect(useInbox.getState().unread).toBe(0);
  });

  it("applies the mk() defaults for fields not overridden", async () => {
    const { maybeSeedDemo, useTransfers } = await freshModules();
    maybeSeedDemo();
    // d1 overrides percent/speed but leaves totalSize/percent set explicitly;
    // t1 uses default totalSize (0) and speedBps (0).
    const t1 = useTransfers.getState().transfers.t1;
    expect(t1.totalSize).toBe(0);
    expect(t1.speedBps).toBe(0);
    expect(Array.isArray(t1.hist)).toBe(true);
  });

  it("is idempotent / guarded on subsequent calls within the same module", async () => {
    const { maybeSeedDemo, useTransfers } = await freshModules();
    maybeSeedDemo();
    expect(Object.keys(useTransfers.getState().transfers)).toHaveLength(6);

    // Clear the store, then call again: the seeded guard must prevent re-seeding.
    useTransfers.setState({ transfers: {}, incomings: [] });
    maybeSeedDemo();
    expect(useTransfers.getState().transfers).toEqual({});
    expect(useTransfers.getState().incomings).toEqual([]);
  });

  it("uses explicit timestamps relative to Date.now() for the send record", async () => {
    vi.useFakeTimers();
    const now = 1_700_000_000_000;
    vi.setSystemTime(now);
    const { maybeSeedDemo, useTransfers } = await freshModules();
    maybeSeedDemo();
    // d1's startedAt is now - 60_000 per the seed data.
    expect(useTransfers.getState().transfers.d1.startedAt).toBe(now - 60_000);
    vi.useRealTimers();
  });
});
