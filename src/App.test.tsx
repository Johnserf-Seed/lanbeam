// The route table, rendered for real (HashRouter + AppShell + pages, browser
// mode — every bridge call is a demo stub).
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import i18n from "./i18n";
import { render, screen, waitFor } from "./test/render";

afterEach(() => {
  window.location.hash = "";
});

describe("App routes", () => {
  it("an unknown path lands on Devices instead of blanking the whole app", async () => {
    // `#/devices` matched no route, so <Routes> rendered NOTHING — AppShell
    // included, with the sidebar, the header and every backend event listener
    // (an incoming request would then time out unseen).
    window.location.hash = "#/devices";
    render(<App />);
    await waitFor(() => expect(window.location.hash).toBe("#/"));
    // The shell is up: the sidebar's navigation is on screen.
    expect(screen.getAllByText(i18n.t("nav.transfers")).length).toBeGreaterThan(
      0,
    );
  });
});
