// What each `lanbeam://` command does once it reaches the app. The contract is
// in deepLink.ts: surface, pre-fill, navigate — never act.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeDeepLink } from "./deepLinkRoute";
import { useOverlays } from "./store";

const ov0 = { ...useOverlays.getState() };
beforeEach(() => {
  useOverlays.setState(ov0, true);
});

describe("routeDeepLink", () => {
  it("lanbeam://devices goes to the index route — there IS no /devices", () => {
    // It used to navigate to "/devices", which matched no route: <Routes>
    // rendered nothing, AppShell and every event listener with it — a blank app.
    const nav = vi.fn();
    routeDeepLink("lanbeam://devices", nav);
    expect(nav).toHaveBeenCalledWith("/");
  });

  it("the other pages navigate by name", () => {
    const nav = vi.fn();
    for (const cmd of ["transfers", "inbox", "settings"])
      routeDeepLink(`lanbeam://${cmd}`, nav);
    expect(nav.mock.calls).toEqual([["/transfers"], ["/inbox"], ["/settings"]]);
  });

  it("a connect link pre-fills the pairing form's join view — and dials nothing", () => {
    const nav = vi.fn();
    routeDeepLink("lanbeam://connect?a=192.168.1.20:51704", nav);
    expect(useOverlays.getState()).toMatchObject({
      pairOpen: true,
      pairPrefill: { addr: "192.168.1.20:51704" },
    });
    expect(nav).not.toHaveBeenCalled();
  });

  it("a pair link stages itself for the join view", () => {
    const url = "lanbeam://pair?d=x&a=10.0.0.9&c=123456";
    routeDeepLink(url, vi.fn());
    expect(useOverlays.getState()).toMatchObject({
      pairOpen: true,
      pairPrefill: { link: url },
    });
  });

  it("a text link pre-fills quick text", () => {
    routeDeepLink("lanbeam://text?t=hi", vi.fn());
    expect(useOverlays.getState()).toMatchObject({
      qtOpen: true,
      qtPrefill: "hi",
    });
  });

  it("open and anything unknown do nothing at all", () => {
    const nav = vi.fn();
    routeDeepLink("lanbeam://open", nav);
    routeDeepLink("lanbeam://trust?d=attacker", nav);
    routeDeepLink("https://example.com", nav);
    expect(nav).not.toHaveBeenCalled();
    expect(useOverlays.getState()).toMatchObject({
      pairOpen: false,
      qtOpen: false,
    });
  });
});
