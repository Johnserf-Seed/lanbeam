import { describe, expect, it } from "vitest";
import { parseDeepLink } from "./deepLink";

describe("parseDeepLink", () => {
  it("parses pair links, handing the raw URL to the pairing form", () => {
    const url = "lanbeam://pair?d=abc&n=Study&a=192.168.1.20&p=51704&c=041583";
    expect(parseDeepLink(url)).toEqual({ cmd: "pair", url });
  });

  it("parses a text link and hands over the decoded body", () => {
    expect(parseDeepLink("lanbeam://text?t=hello%20world")).toEqual({
      cmd: "text",
      text: "hello world",
    });
  });

  it("parses a connect link's address", () => {
    expect(parseDeepLink("lanbeam://connect?a=192.168.1.20:51704")).toEqual({
      cmd: "connect",
      addr: "192.168.1.20:51704",
    });
  });

  it("parses the pure-navigation commands", () => {
    for (const cmd of ["devices", "transfers", "inbox", "settings", "open"]) {
      expect(parseDeepLink(`lanbeam://${cmd}`)).toEqual({ cmd });
    }
    // A trailing slash is the same link.
    expect(parseDeepLink("lanbeam://inbox/")).toEqual({ cmd: "inbox" });
    // The scheme + command are case-insensitive (the OS may rewrite them).
    expect(parseDeepLink("LANBEAM://Inbox")).toEqual({ cmd: "inbox" });
  });

  it("drops anything that is not ours, and NEVER guesses an unknown command", () => {
    expect(parseDeepLink("https://evil.example/pair?d=x")).toBeNull();
    expect(parseDeepLink("lanbeam://sendfiles?to=everyone")).toBeNull(); // not a command
    expect(parseDeepLink("lanbeam://trust?d=attacker")).toBeNull(); // never
    expect(parseDeepLink("lanbeam://")).toBeNull();
    expect(parseDeepLink("")).toBeNull();
    expect(parseDeepLink("   ")).toBeNull();
  });

  it("requires the parameter a command depends on", () => {
    // A `text` with no body, or a `connect` with no address, is not an intent —
    // opening an empty box would just be a link jiggling the UI.
    expect(parseDeepLink("lanbeam://text")).toBeNull();
    expect(parseDeepLink("lanbeam://text?t=")).toBeNull();
    expect(parseDeepLink("lanbeam://connect")).toBeNull();
    expect(parseDeepLink("lanbeam://connect?a=")).toBeNull();
  });

  it("caps link-supplied values so an untrusted link can't stuff the UI", () => {
    const huge = "x".repeat(20_000);
    const parsed = parseDeepLink(`lanbeam://text?t=${huge}`);
    expect(parsed).not.toBeNull();
    if (parsed?.cmd !== "text") throw new Error("expected a text link");
    expect(parsed.text.length).toBe(4000);

    // An absurd address is refused outright rather than truncated into a
    // half-address the user might not read closely.
    expect(parseDeepLink(`lanbeam://connect?a=${"9".repeat(200)}`)).toBeNull();
  });

  it("exposes NO command that acts on the user's behalf", () => {
    // The security contract, asserted. Every command must be confined to
    // surface / pre-fill / navigate — a link that pairs, connects, sends or
    // trusts is a link an attacker gets to press.
    const acting = [
      "lanbeam://send?to=peer&path=/etc/passwd",
      "lanbeam://trust?d=attacker",
      "lanbeam://share?path=/home",
      "lanbeam://accept?session=1",
      "lanbeam://settings?recvPolicy=all", // navigates; must NOT apply the param
    ];
    for (const url of acting) {
      const link = parseDeepLink(url);
      // Either dropped entirely, or (settings) a bare navigation with no payload.
      expect(link === null || Object.keys(link).length === 1).toBe(true);
    }
  });
});
