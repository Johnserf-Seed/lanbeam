import { describe, expect, it } from "vitest";
import { toneColor, trustDot, withHalo } from "./trustDot";

const dev = (over: Partial<Parameters<typeof trustDot>[0]> = {}) => ({
  trusted: false,
  online: true,
  ...over,
});

describe("trustDot", () => {
  it("keeps the trust colour when a trusted device goes away", () => {
    const here = trustDot(dev({ trusted: true, online: true }));
    const away = trustDot(dev({ trusted: true, online: false }));

    // Solid when it's here, a ring when it's away — but the ring is still ours.
    expect(here.background).toBe("var(--dot-live)");
    expect(away.border).toContain("var(--accent)");
    expect(here.tone).toBe("trusted");
    expect(away.tone).toBe("trusted");
  });

  it("never lets 'away' make a trusted device look untrusted", () => {
    // The whole bug: an asleep laptop you trust used to render identically to
    // a stranger on the LAN. Most of a trust circle is asleep at any moment.
    const trustedAway = trustDot(dev({ trusted: true, online: false }));
    const plainAway = trustDot(dev({ trusted: false, online: false }));

    expect(trustedAway.border).not.toBe(plainAway.border);
    expect(trustedAway.background).not.toBe(plainAway.background);
    expect(trustedAway.tone).not.toBe(plainAway.tone);
  });

  it("never dims anything far enough to read as disabled", () => {
    // An offline device is still draggable, renamable and untrustable.
    for (const d of [
      dev({ online: false }),
      dev({ trusted: true, online: false }),
    ]) {
      expect(trustDot(d).opacity).toBeGreaterThanOrEqual(0.8);
    }
  });

  it("shouts a changed fingerprint — solid red, full opacity, online or not", () => {
    // The loudest thing the app can say. Being asleep is not a reason to whisper it.
    for (const online of [true, false]) {
      for (const trusted of [true, false]) {
        const s = trustDot(dev({ trusted, online, fpChanged: { n: "x" } }));
        expect(s.tone).toBe("alert");
        expect(s.background).toBe("var(--danger)");
        expect(s.border).toBe("none"); // never hollow
        expect(s.opacity).toBe(1); // never dimmed
      }
    }
  });

  it("marks an untrusted-but-present device as plain, not absent", () => {
    const s = trustDot(dev({ trusted: false, online: true }));
    expect(s).toMatchObject({
      tone: "plain",
      background: "var(--muted)",
      border: "none",
      opacity: 1,
    });
  });

  it("honours the caller's ring width", () => {
    expect(trustDot(dev({ online: false }), { ringWidth: 2 }).border).toBe(
      "2px solid var(--muted)",
    );
    expect(trustDot(dev({ online: false })).border).toBe(
      "1.5px solid var(--muted)",
    );
  });
});

describe("withHalo", () => {
  it("stacks a selection halo on top of the dot's own glow", () => {
    expect(
      withHalo("0 0 10px var(--glow)", "0 0 0 3px var(--accent-soft)"),
    ).toBe("0 0 0 3px var(--accent-soft),0 0 10px var(--glow)");
  });

  it("replaces 'none' rather than emitting an invalid shadow list", () => {
    expect(withHalo("none", "0 0 0 3px var(--accent-soft)")).toBe(
      "0 0 0 3px var(--accent-soft)",
    );
    expect(withHalo("none", null)).toBe("none");
    expect(withHalo("0 0 10px var(--glow)", null)).toBe("0 0 10px var(--glow)");
  });
});

describe("toneColor", () => {
  it("matches text to the dot it sits under", () => {
    expect(toneColor("alert")).toBe("var(--danger)");
    expect(toneColor("trusted")).toBe("var(--accent-ink)");
    expect(toneColor("plain")).toBe("var(--muted)");
    // A node's name sits darker than a chip's sub-line.
    expect(toneColor("plain", "var(--muted2)")).toBe("var(--muted2)");
    // ...but the loud tones are not the caller's to soften.
    expect(toneColor("alert", "var(--muted2)")).toBe("var(--danger)");
  });
});
