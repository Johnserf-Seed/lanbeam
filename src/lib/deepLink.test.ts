import { describe, expect, it } from "vitest";
import {
  MAX_PAIR_LINK,
  normalizeAddr,
  normalizePairCode,
  parseDeepLink,
  parseJoinTarget,
  parsePairLink,
} from "./deepLink";

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

    // So is an absurd pair link — no invitation we mint is anywhere near it.
    const long = `lanbeam://pair?a=10.0.0.2&c=111111&n=${"x".repeat(MAX_PAIR_LINK)}`;
    expect(parseDeepLink(long)).toBeNull();
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

describe("parsePairLink", () => {
  const ID = "Hst0_-abcdefghijklmnopqrstuvwxyzABCDEFGHIJK"; // 43 chars

  it("unpacks the address, code, name and pin", () => {
    const link = `lanbeam://pair?d=${ID}&n=${encodeURIComponent("书房 · MacBook")}&a=192.168.1.20&p=51704&c=482913`;
    expect(parsePairLink(link)).toEqual({
      addr: "192.168.1.20:51704",
      code: "482913",
      name: "书房 · MacBook",
      deviceId: ID,
    });
  });

  it("mirrors the backend's leniency: bad p= is the default port, bad d= no pin", () => {
    expect(
      parsePairLink("lanbeam://pair?d=abc&a=10.0.0.2&p=nope&c=111111"),
    ).toEqual({
      addr: "10.0.0.2:51704",
      code: "111111",
      name: "",
      deviceId: "",
    });
  });

  it("keeps what a link without an address still carries", () => {
    expect(parsePairLink("lanbeam://pair?p=51704&c=111111")).toMatchObject({
      addr: null,
      code: "111111",
    });
  });

  it("is case-blind about the scheme and the command, like the OS may be", () => {
    expect(parsePairLink("LANBEAM://Pair/?a=10.0.0.2&c=1")?.addr).toBe(
      "10.0.0.2:51704",
    );
  });

  it("is ONLY for pair links", () => {
    expect(parsePairLink("lanbeam://connect?a=10.0.0.2")).toBeNull();
    expect(parsePairLink("https://example.com/pair?a=10.0.0.2")).toBeNull();
    expect(parsePairLink("10.0.0.2:51704")).toBeNull();
    expect(parsePairLink("")).toBeNull();
  });

  it("caps the name by character, never splitting one", () => {
    const n = "😀".repeat(80);
    const got = parsePairLink(
      `lanbeam://pair?a=10.0.0.2&n=${encodeURIComponent(n)}`,
    );
    expect(Array.from(got?.name ?? "")).toHaveLength(64);
    expect(got?.name).toBe("😀".repeat(64));
  });
});

describe("normalizePairCode", () => {
  it("reads a code typed the way it is displayed", () => {
    expect(normalizePairCode("482 913")).toBe("482913");
    expect(normalizePairCode("482-913")).toBe("482913");
    expect(normalizePairCode("  482913 ")).toBe("482913");
  });

  it("folds full-width digits from a CJK input method", () => {
    expect(normalizePairCode("４８２９１３")).toBe("482913");
    expect(normalizePairCode("４８２ 913")).toBe("482913");
  });

  it("keeps at most six digits and nothing else", () => {
    expect(normalizePairCode("1234567")).toBe("123456");
    expect(normalizePairCode("abc")).toBe("");
  });
});

describe("normalizeAddr / parseJoinTarget", () => {
  it("drops spaces and folds a CJK input method's punctuation", () => {
    expect(normalizeAddr(" 192.168.1.20 : 51704 ")).toBe("192.168.1.20:51704");
    expect(normalizeAddr("１９２。１６８．１．２０：５１７０４")).toBe(
      "192.168.1.20:51704",
    );
  });

  it("parses ip and ip:port, defaulting the port", () => {
    expect(parseJoinTarget("10.0.0.2")).toEqual({
      ip: "10.0.0.2",
      port: 51704,
    });
    expect(parseJoinTarget("10.0.0.2:51999")).toEqual({
      ip: "10.0.0.2",
      port: 51999,
    });
  });

  it("refuses exactly what Rust's Ipv4Addr / u16 parsing would", () => {
    for (const bad of [
      "",
      "pc.local",
      "256.0.0.1",
      "10.0.0",
      "10.0.0.01", // leading zero
      "10.0.0.1:0",
      "10.0.0.1:65536",
      "10.0.0.1:",
      "::1",
    ])
      expect(parseJoinTarget(bad), bad).toBeNull();
  });
});
