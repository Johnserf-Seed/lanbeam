// The pairing form's state machine, transition by transition, with no DOM. The
// races these rules exist for are written out as sequences of actions — the
// way they actually interleave between two machines.
import { beforeEach, describe, expect, it } from "vitest";
import type { ConnectResult, PairingInvite } from "../bridge/api";
import i18n from "../i18n";
import {
  type PairAction,
  type PairState,
  fmtLinkPreview,
  fmtPairCode,
  initialPairState,
  joinErrorOf,
  pairReducer,
  validateAddr,
  validateJoin,
} from "./pairing";

const INVITE: PairingInvite = {
  code: "482913",
  qr: "lanbeam://pair?d=x&n=Study&a=192.168.1.20&p=51704&c=482913",
  addr: "192.168.1.20:51704",
  expiresAt: 1_000_000,
};
const PEER: ConnectResult = { deviceId: "p1", name: "Pixel", sas: "483921" };
const OTHER: ConnectResult = {
  deviceId: "j1",
  name: "ThinkPad",
  sas: "246810",
};
const HOST_ID = "Hst0_-abcdefghijklmnopqrstuvwxyzABCDEFGHIJK";
const LINK = `lanbeam://pair?d=${HOST_ID}&n=Study&a=192.168.1.9&p=51999&c=135790`;

const run = (s: PairState, ...actions: PairAction[]) =>
  actions.reduce(pairReducer, s);

/** Open, without a prefill (the host view), at epoch 1. */
const opened = () =>
  run(initialPairState(), { type: "open", epoch: 1, prefill: null });
/** Opened, with a live code at epoch 2. */
const hosting = () =>
  run(
    opened(),
    { type: "mintStart", epoch: 2 },
    { type: "minted", epoch: 2, invite: INVITE },
  );
/** Hosting, then switched to the join view with a valid form filled in. */
const filled = () =>
  run(
    hosting(),
    { type: "role", role: "join" },
    { type: "addr", value: "192.168.1.9" },
    { type: "code", value: "135790" },
  );

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("pairReducer — opening", () => {
  it("opens on the host view with nothing filled in", () => {
    const s = opened();
    expect(s).toMatchObject({
      open: true,
      epoch: 1,
      role: "host",
      invite: null,
    });
  });

  it("a link prefill opens the JOIN view, unpacked and pinned", () => {
    const s = run(initialPairState(), {
      type: "open",
      epoch: 1,
      prefill: { link: LINK },
    });
    expect(s).toMatchObject({
      role: "join",
      addr: "192.168.1.9:51999",
      code: "135790",
      source: { kind: "link", name: "Study" },
      expectedId: HOST_ID,
    });
  });

  it("an address prefill opens the join view with the address only", () => {
    const s = run(initialPairState(), {
      type: "open",
      epoch: 1,
      prefill: { addr: "10.0.0.5:51704", name: "NAS" },
    });
    expect(s).toMatchObject({
      role: "join",
      addr: "10.0.0.5:51704",
      code: "",
      source: { kind: "addr", name: "NAS" },
      expectedId: null,
    });
  });

  it("close resets everything to a fresh epoch", () => {
    const s = run(filled(), { type: "close", epoch: 9 });
    expect(s).toEqual(initialPairState(9));
  });
});

describe("pairReducer — minting", () => {
  it("applies a mint only under the epoch it started in", () => {
    // 换一个 while the first mint is in flight: the first result is stale.
    const s = run(
      opened(),
      { type: "mintStart", epoch: 2 },
      { type: "mintStart", epoch: 3 },
      { type: "minted", epoch: 2, invite: { ...INVITE, code: "111111" } },
    );
    expect(s.invite).toBeNull();
    expect(s.minting).toBe(true);
    const t = run(s, { type: "minted", epoch: 3, invite: INVITE });
    expect(t.invite?.code).toBe("482913");
  });

  it("a stale failure doesn't fail the fresh mint", () => {
    const s = run(
      opened(),
      { type: "mintStart", epoch: 2 },
      { type: "mintStart", epoch: 3 },
      { type: "mintFailed", epoch: 2 },
    );
    expect(s).toMatchObject({ minting: true, mintFailed: false });
  });

  it("refuses to mint while a join is in flight or a SAS is on screen", () => {
    const joining = run(filled(), { type: "joinStart", epoch: 5 });
    expect(run(joining, { type: "mintStart", epoch: 6 }).epoch).toBe(5);
    const comparing = run(joining, { type: "joinDone", epoch: 5, peer: PEER });
    expect(run(comparing, { type: "mintStart", epoch: 6 }).epoch).toBe(5);
  });
});

describe("pairReducer — the two roles", () => {
  it("switching tabs leaves the code alone", () => {
    const s = run(
      hosting(),
      { type: "role", role: "join" },
      { type: "role", role: "host" },
    );
    expect(s.invite).toEqual(INVITE);
    expect(s.epoch).toBe(2);
  });

  it("pressing Join drops the host code and starts a new epoch", () => {
    const s = run(filled(), { type: "joinStart", epoch: 5 });
    expect(s).toMatchObject({ busy: "join", invite: null, epoch: 5 });
  });

  it("a join in flight pins the view and the fields", () => {
    const s = run(filled(), { type: "joinStart", epoch: 5 });
    const t = run(
      s,
      { type: "role", role: "host" },
      { type: "addr", value: "10.9.9.9" },
      { type: "code", value: "000000" },
      { type: "prefill", prefill: { link: LINK } },
      { type: "clearSource" },
    );
    expect(t).toEqual(s);
  });

  it("a join result lands only in its own epoch", () => {
    const s = run(filled(), { type: "joinStart", epoch: 5 });
    // Closed meanwhile (a new epoch): the late result is dropped.
    const closed = run(
      s,
      { type: "close", epoch: 6 },
      { type: "open", epoch: 7, prefill: null },
    );
    expect(
      run(closed, { type: "joinDone", epoch: 5, peer: PEER }).compare,
    ).toBeNull();
    // Still current: it lands.
    expect(run(s, { type: "joinDone", epoch: 5, peer: PEER }).compare).toEqual({
      ...PEER,
      via: "join",
    });
  });

  it("someone redeeming our code puts the host on the compare step", () => {
    const s = run(hosting(), { type: "joined", peer: OTHER });
    expect(s.compare).toEqual({ ...OTHER, via: "host" });
  });

  it("…from the join tab too: a code is redeemable until Join is pressed", () => {
    const s = run(
      hosting(),
      { type: "role", role: "join" },
      { type: "joined", peer: OTHER },
    );
    expect(s.compare).toEqual({ ...OTHER, via: "host" });
  });

  it("a pair_joined during our own join is stashed, and wins if our join fails", () => {
    const s = run(
      filled(),
      { type: "joinStart", epoch: 5 },
      { type: "joined", peer: OTHER },
    );
    expect(s.compare).toBeNull();
    expect(s.stashed).toEqual({ ...OTHER, via: "host" });
    const failed = run(s, {
      type: "joinFailed",
      epoch: 5,
      error: { field: "code", key: "pair.rejected" },
    });
    expect(failed).toMatchObject({
      busy: null,
      error: null,
      stashed: null,
      compare: { ...OTHER, via: "host" },
    });
  });

  it("…and if our join succeeds, ours is shown and the stash dropped", () => {
    const s = run(
      filled(),
      { type: "joinStart", epoch: 5 },
      { type: "joined", peer: OTHER },
      { type: "joinDone", epoch: 5, peer: PEER },
    );
    expect(s.compare).toEqual({ ...PEER, via: "join" });
    expect(s.stashed).toBeNull();
  });

  it("nothing replaces a SAS that is already on screen", () => {
    const s = run(
      hosting(),
      { type: "joined", peer: OTHER },
      { type: "joined", peer: PEER },
    );
    expect(s.compare).toEqual({ ...OTHER, via: "host" });
  });

  it("a pair_joined after close is ignored", () => {
    const s = run(
      hosting(),
      { type: "close", epoch: 3 },
      { type: "joined", peer: OTHER },
    );
    expect(s.compare).toBeNull();
  });
});

describe("pairReducer — the join fields", () => {
  it("a pasted link fills both fields, from either field", () => {
    for (const type of ["addr", "code"] as const) {
      const s = run(
        opened(),
        { type: "role", role: "join" },
        { type, value: LINK },
      );
      expect(s).toMatchObject({
        addr: "192.168.1.9:51999",
        code: "135790",
        expectedId: HOST_ID,
      });
    }
  });

  it("typing `lanbeam://` by hand does not wipe the field halfway", () => {
    const s = run(opened(), { type: "addr", value: "lanbeam://pair" });
    expect(s.addr).toBe("lanbeam://pair");
    expect(s.source).toBeNull();
  });

  it("the code field keeps only digits, at most six", () => {
    const s = run(opened(), { type: "code", value: "48-29 13 7" });
    expect(s.code).toBe("482913");
  });

  it("typing in one field clears that field's error, not the other's", () => {
    const s = run(opened(), {
      type: "error",
      error: { field: "addr", key: "pair.badAddr" },
    });
    expect(run(s, { type: "code", value: "1" }).error?.field).toBe("addr");
    expect(run(s, { type: "addr", value: "1" }).error).toBeNull();
  });

  it("an add in flight keeps the code alive, and reports in its epoch", () => {
    const s = run(filled(), { type: "addStart" });
    expect(s).toMatchObject({ busy: "add", invite: INVITE, epoch: 2 });
    const done = run(s, { type: "addDone", epoch: 2 });
    expect(done.busy).toBeNull();
    // Closed meanwhile: the late result changes nothing.
    const closed = run(s, { type: "close", epoch: 3 });
    expect(run(closed, { type: "addDone", epoch: 2 })).toEqual(closed);
  });
});

describe("validateAddr / validateJoin", () => {
  it("accepts ip and ip:port, defaulting the port", () => {
    expect(validateAddr("192.168.1.8")).toEqual({
      ok: true,
      addr: "192.168.1.8:51704",
    });
    expect(validateAddr(" 10.0.0.2 : 51999 ")).toEqual({
      ok: true,
      addr: "10.0.0.2:51999",
    });
    // A CJK input method's full-width colon / digits / full stop.
    expect(validateAddr("１９２。１６８。１．８：５１７０４")).toEqual({
      ok: true,
      addr: "192.168.1.8:51704",
    });
  });

  it("calls the classic mix-up by its name", () => {
    expect(validateAddr("482913")).toMatchObject({
      error: { key: "pair.codeMixup" },
    });
    expect(validateAddr("482 913")).toMatchObject({
      error: { key: "pair.codeMixup" },
    });
  });

  it("refuses what the backend would refuse", () => {
    for (const bad of [
      "",
      "my-pc",
      "999.1.1.1",
      "192.168.01.1",
      "1.2.3.4:0",
      "1.2.3.4:70000",
      "1.2.3",
    ])
      expect(validateAddr(bad), bad).toMatchObject({
        ok: false,
        error: { field: "addr", key: "pair.badAddr" },
      });
  });

  it("checks the address first, then the code", () => {
    expect(validateJoin("x", "")).toMatchObject({ error: { field: "addr" } });
    expect(validateJoin("10.0.0.2", "")).toMatchObject({
      error: { field: "code", key: "errors.pairCodeMissing" },
    });
    expect(validateJoin("10.0.0.2", "48291")).toMatchObject({
      error: { field: "code", key: "pair.codeShort" },
    });
    expect(validateJoin("10.0.0.2", "482 913")).toEqual({
      ok: true,
      addr: "10.0.0.2:51704",
      code: "482913",
    });
  });
});

describe("joinErrorOf", () => {
  it("puts each failure next to its cause", () => {
    expect(joinErrorOf({ kind: "PairRejected" }, "a")).toMatchObject({
      field: "code",
      key: "pair.rejected",
    });
    expect(joinErrorOf({ kind: "PairCodeMissing" }, "a")).toMatchObject({
      field: "code",
    });
    expect(joinErrorOf({ kind: "InvalidAddress" }, "a")).toMatchObject({
      field: "addr",
    });
    expect(joinErrorOf({ kind: "Unreachable" }, "10.0.0.2:51704")).toEqual({
      field: "form",
      key: "pair.unreachable",
      params: { addr: "10.0.0.2:51704" },
    });
    expect(joinErrorOf({ kind: "Timeout" }, "a")).toMatchObject({
      key: "pair.unreachable",
    });
    expect(joinErrorOf({ kind: "IdentityMismatch" }, "a")).toMatchObject({
      key: "pair.identityMismatch",
    });
  });

  it("words anything else with the caller's fallback and the localized reason", () => {
    expect(joinErrorOf({ kind: "SelfPeer" }, "a")).toEqual({
      field: "form",
      key: "pair.joinFailed",
      params: { err: i18n.t("errors.selfPeer") },
    });
    expect(
      joinErrorOf({ kind: "Handshake" }, "a", "devices.ipFailed"),
    ).toMatchObject({
      key: "devices.ipFailed",
    });
  });
});

describe("fmtPairCode / fmtLinkPreview", () => {
  it("groups the code the way the host displays it", () => {
    expect(fmtPairCode("482913")).toBe("482 913");
    expect(fmtPairCode("4829")).toBe("482 9");
    expect(fmtPairCode("48")).toBe("48");
    expect(fmtPairCode("")).toBe("");
  });

  it("shows a link as what it is: the address and the code", () => {
    expect(fmtLinkPreview(INVITE.qr)).toBe("…a=192.168.1.20&p=51704&c=482913");
    expect(fmtLinkPreview("lanbeam://pair?d=x&p=51704&c=1")).toBe("");
  });
});
