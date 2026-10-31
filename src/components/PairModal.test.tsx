// Component tests for PairModal. The pairing commands are mocked so each test
// controls exactly when (and how) start_pairing / join_by_code settle, and
// `pair_joined` is fired by hand. Text is queried through i18n.t(key) in
// English, so the assertions follow the copy rather than duplicating it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../bridge/api";
import type { ConnectResult, PairingInvite } from "../bridge/api";
import i18n from "../i18n";
import * as sendops from "../lib/sendops";
import {
  type PairPrefill,
  useData,
  useOverlays,
  useToast,
  useTrust,
} from "../lib/store";
import { act, fireEvent, renderUI, screen, waitFor } from "../test/render";
import PairModal from "./PairModal";

const h = vi.hoisted(() => ({
  listeners: new Map<string, Set<(p: unknown) => void>>(),
}));

vi.mock("../bridge/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../bridge/api")>();
  return {
    ...actual,
    startPairing: vi.fn(),
    cancelPairing: vi.fn(),
    joinByCode: vi.fn(),
    connectByAddr: vi.fn(),
    listDiscoveredDevices: vi.fn(),
    onEvent: vi.fn((name: string, cb: (p: unknown) => void) => {
      let set = h.listeners.get(name);
      if (!set) {
        set = new Set();
        h.listeners.set(name, set);
      }
      set.add(cb);
      return () => {
        set.delete(cb);
      };
    }),
  };
});

// Spy on copyText but keep the rest of sendops (errText etc.) real.
vi.mock("../lib/sendops", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/sendops")>();
  return { ...actual, copyText: vi.fn().mockResolvedValue(true) };
});

const T = (k: string, o?: Record<string, unknown>) => i18n.t(k, o);

const LINK = "lanbeam://pair?d=demo&n=Study&a=192.168.1.20&p=51704&c=482913";
const invite = (over: Partial<PairingInvite> = {}): PairingInvite => ({
  code: "482913",
  qr: LINK,
  addr: "192.168.1.20:51704",
  expiresAt: Date.now() + 600_000,
  ...over,
});
const PEER: ConnectResult = {
  deviceId: "peer-1",
  name: "Pixel 8 Pro",
  sas: "483921",
};
/** A real-shaped Device ID (43 base64url chars) — the kind `d=` pins to. */
const HOST_ID = "Hst0_-abcdefghijklmnopqrstuvwxyzABCDEFGHIJK";
const HOST_LINK = `lanbeam://pair?d=${HOST_ID}&n=${encodeURIComponent(
  "书房 · MacBook",
)}&a=192.168.1.9&p=51999&c=135790`;

function deferred<V>() {
  let resolve!: (v: V) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<V>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Fire a backend event at whoever is listening. */
const emit = (name: string, payload: unknown) =>
  act(() => {
    for (const cb of h.listeners.get(name) ?? []) cb(payload);
  });

const overlays0 = { ...useOverlays.getState() };
const toast0 = { ...useToast.getState() };
const trust0 = { ...useTrust.getState() };

beforeEach(async () => {
  await i18n.changeLanguage("en");
  useOverlays.setState(overlays0, true);
  useToast.setState(toast0, true);
  useTrust.setState({ ...trust0, records: {} }, true);
  h.listeners.clear();
  vi.mocked(api.startPairing)
    .mockReset()
    .mockImplementation(() => Promise.resolve(invite()));
  vi.mocked(api.cancelPairing).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.joinByCode).mockReset().mockResolvedValue(PEER);
  vi.mocked(api.connectByAddr)
    .mockReset()
    .mockResolvedValue({ deviceId: "added-1", name: "NAS", sas: "111222" });
  vi.mocked(api.listDiscoveredDevices).mockReset().mockResolvedValue([]);
  vi.mocked(sendops.copyText).mockClear();
});

function mount(prefill: PairPrefill | null = null) {
  useOverlays.getState().setPair(true, prefill);
  return renderUI(<PairModal />);
}
const tab = (key: "pair.roleHost" | "pair.roleJoin") =>
  screen.getByRole("button", { name: T(key) });
const addrField = () =>
  screen.getByLabelText(T("pair.joinAddr")) as HTMLInputElement;
const codeField = () =>
  screen.getByLabelText(T("pair.joinCode")) as HTMLInputElement;
const joinBtn = () => screen.getByRole("button", { name: T("pair.joinBtn") });
const type = (el: HTMLElement, value: string) =>
  fireEvent.change(el, { target: { value } });

/** Fill the join form and press Join. */
function join(addr = "192.168.1.20", code = "482913") {
  type(addrField(), addr);
  type(codeField(), code);
  fireEvent.click(joinBtn());
}

/** Drive the joiner half to the compare step (the mock resolves PEER). */
async function joinToCompare() {
  mount();
  fireEvent.click(tab("pair.roleJoin"));
  join();
  await screen.findByText(T("pair.match"));
}

describe("PairModal", () => {
  it("renders nothing while closed", () => {
    const { container } = renderUI(<PairModal />);
    expect(container).toBeEmptyDOMElement();
  });

  describe("让对方加入我 — the host view", () => {
    it("shows this device's address AND the code: the two things the other side types", async () => {
      mount();
      expect(await screen.findByText("482 913")).toBeInTheDocument();
      expect(screen.getByText("192.168.1.20")).toBeInTheDocument();
      expect(screen.getByText(":51704")).toBeInTheDocument();
      expect(screen.getByText(T("pair.hostLead"))).toBeInTheDocument();
      expect(screen.getByText(T("pair.waiting"))).toBeInTheDocument();
      // The link is shown for what it is — the same address and code, packed.
      expect(
        screen.getByText("…a=192.168.1.20&p=51704&c=482913"),
      ).toBeInTheDocument();
      // No QR: LanBeam has no scanner, and a phone can't pair.
      expect(screen.queryByRole("img", { name: "QR" })).toBeNull();
      expect(api.startPairing).toHaveBeenCalledTimes(1);
    });

    it("copies the address, the code and the link — each toast says which", async () => {
      mount();
      await screen.findByText("482 913");
      const copy = (what: string) =>
        screen.getByRole("button", { name: `${T("common.copy")} · ${what}` });

      fireEvent.click(copy(T("pair.addrLabel")));
      expect(sendops.copyText).toHaveBeenLastCalledWith("192.168.1.20:51704");
      await waitFor(() =>
        expect(useToast.getState().msg).toBe(T("pair.addrCopied")),
      );

      fireEvent.click(copy(T("pair.codeLabel")));
      expect(sendops.copyText).toHaveBeenLastCalledWith("482913");
      await waitFor(() =>
        expect(useToast.getState().msg).toBe(T("pair.codeCopied")),
      );

      fireEvent.click(screen.getByRole("button", { name: T("pair.copyLink") }));
      expect(sendops.copyText).toHaveBeenLastCalledWith(LINK);
      await waitFor(() =>
        expect(useToast.getState().msg).toBe(T("pair.linkCopied")),
      );

      // A copy that didn't happen doesn't say it did (M24).
      vi.mocked(sendops.copyText).mockResolvedValueOnce(false);
      fireEvent.click(copy(T("pair.codeLabel")));
      await waitFor(() =>
        expect(useToast.getState().msg).toBe(T("common.copyFailed")),
      );
    });

    it("with no LAN address, says so — and offers no link that couldn't work", async () => {
      vi.mocked(api.startPairing).mockResolvedValue(
        invite({
          addr: null,
          qr: "lanbeam://pair?d=demo&n=Study&p=51704&c=482913",
        }),
      );
      mount();
      expect(await screen.findByText(T("pair.noLan"))).toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: T("pair.copyLink") }),
      ).toBeNull();
      expect(screen.queryByText(T("pair.waiting"))).toBeNull();
    });

    describe("the code's lifetime (fake clock)", () => {
      beforeEach(() => {
        vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
        vi.setSystemTime(new Date("2026-10-30T10:00:00Z"));
      });
      afterEach(() => {
        vi.useRealTimers();
      });

      it("counts down, then says it expired and offers a new one", async () => {
        mount();
        await screen.findByText("482 913");
        expect(
          screen.getByText(T("pair.expiresIn", { t: "10:00" })),
        ).toBeInTheDocument();

        act(() => {
          vi.advanceTimersByTime(19_000);
        });
        expect(
          screen.getByText(T("pair.expiresIn", { t: "9:41" })),
        ).toBeInTheDocument();

        act(() => {
          vi.advanceTimersByTime(600_000);
        });
        expect(screen.getByText(T("pair.expired"))).toBeInTheDocument();
        // Nothing on screen still pretends the old code works.
        expect(screen.queryByText(T("pair.waiting"))).toBeNull();
        expect(
          screen.queryByRole("button", { name: T("pair.copyLink") }),
        ).toBeNull();

        vi.mocked(api.startPairing).mockResolvedValueOnce(
          invite({ code: "135790" }),
        );
        fireEvent.click(screen.getByRole("button", { name: T("pair.regen") }));
        expect(await screen.findByText("135 790")).toBeInTheDocument();
        expect(useToast.getState().msg).toBe(T("pair.regenToast"));
        expect(
          screen.getByText(T("pair.expiresIn", { t: "10:00" })),
        ).toBeInTheDocument();
      });
    });

    it("a mint that lands after a close + reopen can't replace the new code", async () => {
      const slow = deferred<PairingInvite>();
      vi.mocked(api.startPairing)
        .mockImplementationOnce(() => slow.promise)
        .mockImplementationOnce(() =>
          Promise.resolve(invite({ code: "111111" })),
        );
      mount();
      act(() => useOverlays.getState().setPair(false));
      act(() => useOverlays.getState().setPair(true));
      expect(await screen.findByText("111 111")).toBeInTheDocument();

      await act(async () => slow.resolve(invite({ code: "222222" })));
      expect(screen.getByText("111 111")).toBeInTheDocument();
      expect(screen.queryByText("222 222")).toBeNull();
    });
  });

  describe("加入对方 — the join view", () => {
    it("switching tabs keeps the code alive; pressing Join cancels it before dialing", async () => {
      mount();
      await screen.findByText("482 913");
      fireEvent.click(tab("pair.roleJoin"));
      fireEvent.click(tab("pair.roleHost"));
      // Same code, never cancelled, never re-minted: a code already read out
      // loud must not die of a misclick.
      expect(screen.getByText("482 913")).toBeInTheDocument();
      expect(api.cancelPairing).not.toHaveBeenCalled();
      expect(api.startPairing).toHaveBeenCalledTimes(1);

      fireEvent.click(tab("pair.roleJoin"));
      join();
      await waitFor(() => expect(api.joinByCode).toHaveBeenCalled());
      // A device that is joining someone can't also be redeemed as a host.
      expect(
        vi.mocked(api.cancelPairing).mock.invocationCallOrder[0],
      ).toBeLessThan(vi.mocked(api.joinByCode).mock.invocationCallOrder[0]);
    });

    it("takes the code the way the host DISPLAYS it, and full-width input", async () => {
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      type(codeField(), "482 913");
      expect(codeField().value).toBe("482 913");
      join("192.168.1.20：51704", "４８２９１３");
      await waitFor(() =>
        expect(api.joinByCode).toHaveBeenCalledWith(
          "192.168.1.20:51704",
          "482913",
          undefined,
        ),
      );
    });

    it.each([
      ["a 5-digit code", "192.168.1.20", "48291", "pair.codeShort"],
      ["no code at all", "192.168.1.20", "", "errors.pairCodeMissing"],
      ["the code in the address field", "482913", "482913", "pair.codeMixup"],
      ["a non-address", "my-laptop", "482913", "pair.badAddr"],
    ])(
      "catches %s next to the field, before anything is sent",
      async (_what, addr, code, key) => {
        mount();
        fireEvent.click(tab("pair.roleJoin"));
        join(addr, code);
        expect(screen.getByRole("alert")).toHaveTextContent(T(key));
        expect(api.joinByCode).not.toHaveBeenCalled();
        // Nothing was even cancelled: the host code survives a typo.
        expect(api.cancelPairing).not.toHaveBeenCalled();
      },
    );

    it("unpacks a pasted pair link into both fields — and pins the join to the link's device", async () => {
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      type(addrField(), HOST_LINK);
      expect(addrField().value).toBe("192.168.1.9:51999");
      expect(codeField().value).toBe("135 790");
      expect(
        screen.getByText(T("pair.linkFrom", { name: "书房 · MacBook" })),
      ).toBeInTheDocument();

      fireEvent.click(joinBtn());
      await waitFor(() =>
        expect(api.joinByCode).toHaveBeenCalledWith(
          "192.168.1.9:51999",
          "135790",
          HOST_ID,
        ),
      );
    });

    it("unpacks a link pasted into the CODE field too, and 清除 forgets it", () => {
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      type(codeField(), HOST_LINK);
      expect(addrField().value).toBe("192.168.1.9:51999");
      expect(codeField().value).toBe("135 790");

      fireEvent.click(
        screen.getByRole("button", { name: T("pair.linkClear") }),
      );
      expect(addrField().value).toBe("");
      expect(codeField().value).toBe("");
      expect(screen.getByText(T("pair.joinPasteHint"))).toBeInTheDocument();
    });

    it("a deep link opens straight on 加入对方, filled in — and mints no code", () => {
      mount({ link: HOST_LINK });
      expect(tab("pair.roleJoin")).toHaveAttribute("aria-pressed", "true");
      expect(addrField().value).toBe("192.168.1.9:51999");
      expect(codeField().value).toBe("135 790");
      expect(api.startPairing).not.toHaveBeenCalled();
    });

    it("an address prefill (the fingerprint alert) names whose address it is", () => {
      mount({ addr: "192.168.1.9:51999", name: "客厅 · Mac mini" });
      expect(addrField().value).toBe("192.168.1.9:51999");
      expect(codeField().value).toBe("");
      expect(
        screen.getByText(T("pair.addrFrom", { name: "客厅 · Mac mini" })),
      ).toBeInTheDocument();
    });

    it("a link opened while the form is up switches to 加入对方 and fills it in", async () => {
      mount();
      await screen.findByText("482 913");
      act(() => useOverlays.getState().setPair(true, { link: HOST_LINK }));
      expect(tab("pair.roleJoin")).toHaveAttribute("aria-pressed", "true");
      expect(addrField().value).toBe("192.168.1.9:51999");
      // Like a tab switch, it leaves the host code alone.
      expect(api.cancelPairing).not.toHaveBeenCalled();
    });

    it("…but not in the middle of a join: it says so and changes nothing", async () => {
      vi.mocked(api.joinByCode).mockImplementation(
        () => deferred<ConnectResult>().promise,
      );
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      join();
      await waitFor(() => expect(api.joinByCode).toHaveBeenCalled());
      expect(
        screen.getByRole("button", { name: T("pair.joining") }),
      ).toBeDisabled();

      act(() => useOverlays.getState().setPair(true, { link: HOST_LINK }));
      expect(useToast.getState().msg).toBe(T("pair.busyPrefill"));
      expect(addrField().value).toBe("192.168.1.20");
    });

    it.each([
      ["PairRejected", "pair.rejected", "code"],
      ["InvalidAddress", "errors.invalidAddress", "addr"],
      ["Unreachable", "pair.unreachable", null],
      ["Timeout", "pair.unreachable", null],
      ["IdentityMismatch", "pair.identityMismatch", null],
    ])(
      "says what a %s means, next to what caused it",
      async (kind, key, field) => {
        vi.mocked(api.joinByCode).mockRejectedValue({
          kind,
          message: "internal diagnostics",
        });
        mount();
        fireEvent.click(tab("pair.roleJoin"));
        join();
        const alert = await screen.findByRole("alert");
        expect(alert).toHaveTextContent(T(key, { addr: "192.168.1.20:51704" }));
        // The backend's message is diagnostics, never UI.
        expect(screen.queryByText(/internal diagnostics/)).toBeNull();
        if (field === "code")
          expect(codeField()).toHaveAttribute("aria-invalid", "true");
        if (field === "addr")
          expect(addrField()).toHaveAttribute("aria-invalid", "true");
        // Join is available again for the next try.
        expect(joinBtn()).not.toBeDisabled();
      },
    );

    it("anything else falls back to 配对失败 with the localized reason", async () => {
      vi.mocked(api.joinByCode).mockRejectedValue({ kind: "SelfPeer" });
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      join();
      expect(await screen.findByRole("alert")).toHaveTextContent(
        T("pair.joinFailed", { err: T("errors.selfPeer") }),
      );
    });

    it("after a join, refreshes the device list so the peer shows up", async () => {
      const list = [
        {
          deviceId: PEER.deviceId,
          name: PEER.name,
          address: "192.168.1.20",
          port: 51704,
          manual: true,
        },
      ];
      vi.mocked(api.listDiscoveredDevices).mockResolvedValue(list);
      await joinToCompare();
      await waitFor(() => expect(useData.getState().devices).toEqual(list));
    });

    it("只添加设备 adds the device without pairing, and closes", async () => {
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      type(addrField(), "192.168.1.30");
      fireEvent.click(screen.getByRole("button", { name: T("pair.addOnly") }));
      await waitFor(() => expect(useOverlays.getState().pairOpen).toBe(false));
      expect(api.connectByAddr).toHaveBeenCalledWith("192.168.1.30:51704");
      expect(api.joinByCode).not.toHaveBeenCalled();
      expect(useToast.getState().msg).toBe(
        T("devices.ipConnected", { name: "NAS" }),
      );
    });
  });

  describe("the SAS compare step", () => {
    it("shows the joiner the SAS instead of quietly closing", async () => {
      // The whole point: the side that redeems the code sees the SAME number the
      // host is showing. A code only one end can read is not a check at all.
      await joinToCompare();
      expect(screen.getByText("483 · 921")).toBeInTheDocument(); // fmtSas
      expect(
        screen.getByText(T("pair.compareTitleJoin", { name: PEER.name })),
      ).toBeInTheDocument();
      expect(screen.getByText(T("pair.compareWarn"))).toBeInTheDocument();
    });

    it("the host lands on it when someone redeems its code", async () => {
      mount();
      await screen.findByText("482 913");
      emit("pair_joined", { deviceId: "j1", name: "ThinkPad", sas: "246810" });
      expect(screen.getByText("246 · 810")).toBeInTheDocument();
      expect(
        screen.getByText(T("pair.compareTitleHost", { name: "ThinkPad" })),
      ).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: T("pair.match") }));
      expect(useTrust.getState().records.j1).toMatchObject({ trusted: true });
    });

    it("grants NO trust until the user says the two screens agree", async () => {
      await joinToCompare();
      // Redeeming a valid code proves the code, not the peer.
      expect(useTrust.getState().records[PEER.deviceId]).toBeUndefined();

      fireEvent.click(screen.getByRole("button", { name: T("pair.match") }));
      const rec = useTrust.getState().records[PEER.deviceId];
      // Trust rides the same path as the trust circle — so it comes with
      // auto-accept, exactly like a device dragged into the ring.
      expect(rec).toMatchObject({ trusted: true, autoAccept: true });
      expect(useOverlays.getState().pairOpen).toBe(false);
    });

    it("trusts nothing when the codes don't match, and says why", async () => {
      await joinToCompare();
      fireEvent.click(screen.getByRole("button", { name: T("pair.mismatch") }));
      expect(useTrust.getState().records[PEER.deviceId]).toBeUndefined();
      expect(useToast.getState().msg).toBe(T("pair.mismatchToast"));
    });

    it("trusts nothing when dismissed, and doesn't do it silently", async () => {
      // Walking away is a "no". The dangerous version of this bug is the quiet
      // one: the user closes the box believing they paired.
      await joinToCompare();
      fireEvent.click(screen.getByRole("button", { name: "×" }));
      expect(useTrust.getState().records[PEER.deviceId]).toBeUndefined();
      expect(useToast.getState().msg).toBe(T("pair.unconfirmedToast"));
    });

    it("a pair_joined during the compare step changes nothing", async () => {
      await joinToCompare();
      emit("pair_joined", { deviceId: "j1", name: "ThinkPad", sas: "246810" });
      expect(screen.getByText("483 · 921")).toBeInTheDocument();
      expect(screen.queryByText("246 · 810")).toBeNull();
    });
  });

  describe("the two roles racing", () => {
    it("someone's join into us during our own join is held — and shown if ours fails", async () => {
      // The one race left: their join beat our cancel. Our join then fails (they
      // had already stopped hosting), but THEIR pairing is live — their screen
      // shows its SAS, so ours must too.
      const ours = deferred<ConnectResult>();
      vi.mocked(api.joinByCode).mockImplementation(() => ours.promise);
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      join();
      await waitFor(() => expect(api.joinByCode).toHaveBeenCalled());

      emit("pair_joined", { deviceId: "j1", name: "ThinkPad", sas: "246810" });
      expect(screen.queryByText("246 · 810")).toBeNull();

      await act(async () => ours.reject({ kind: "PairRejected" }));
      expect(screen.getByText("246 · 810")).toBeInTheDocument();
      expect(
        screen.getByText(T("pair.compareTitleHost", { name: "ThinkPad" })),
      ).toBeInTheDocument();
    });

    it("closing mid-join: the late result shows no SAS and trusts nothing", async () => {
      const ours = deferred<ConnectResult>();
      vi.mocked(api.joinByCode).mockImplementation(() => ours.promise);
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      join();
      await waitFor(() => expect(api.joinByCode).toHaveBeenCalled());

      act(() => useOverlays.getState().setPair(false));
      act(() => useOverlays.getState().setPair(true));
      await act(async () => ours.resolve(PEER));

      expect(screen.queryByText("483 · 921")).toBeNull();
      expect(await screen.findByText("482 913")).toBeInTheDocument();
      expect(useTrust.getState().records[PEER.deviceId]).toBeUndefined();
    });

    it("a join closed before it dialed never dials", async () => {
      const cancel = deferred<void>();
      vi.mocked(api.cancelPairing).mockImplementationOnce(() => cancel.promise);
      mount();
      fireEvent.click(tab("pair.roleJoin"));
      join();
      act(() => useOverlays.getState().setPair(false));
      await act(async () => cancel.resolve());
      expect(api.joinByCode).not.toHaveBeenCalled();
    });
  });

  describe("closing", () => {
    it("cancels the code exactly once, and stops listening", async () => {
      mount();
      await screen.findByText("482 913");
      expect(h.listeners.get("pair_joined")?.size).toBe(1);

      fireEvent.click(screen.getByRole("button", { name: "×" }));
      expect(useOverlays.getState().pairOpen).toBe(false);
      expect(api.cancelPairing).toHaveBeenCalledTimes(1);
      expect(h.listeners.get("pair_joined")?.size).toBe(0);
    });

    it("Escape closes it, like the backdrop — but not an IME's Escape", () => {
      mount();
      fireEvent.keyDown(window, { key: "Escape", isComposing: true });
      expect(useOverlays.getState().pairOpen).toBe(true);
      fireEvent.keyDown(window, { key: "Escape" });
      expect(useOverlays.getState().pairOpen).toBe(false);
    });

    it("a reopen starts clean: host view, no stale prefill", async () => {
      mount({ link: HOST_LINK });
      act(() => useOverlays.getState().setPair(false));
      act(() => useOverlays.getState().setPair(true));
      expect(tab("pair.roleHost")).toHaveAttribute("aria-pressed", "true");
      expect(await screen.findByText("482 913")).toBeInTheDocument();
    });
  });
});
