// The update check (F2): it asks GitHub for one thing — the newest release's
// tag — compares it with this build, and turns every way that can go wrong
// into a plain "couldn't check".
import { afterEach, describe, expect, it, vi } from "vitest";
import { checkForUpdate, compareVersions, RELEASES_URL } from "./update";

/** A fetch that answers every request with `status` and `body`. */
const answering = (status: number, body: unknown) =>
  vi.fn(
    async (_url: RequestInfo | URL, _init?: RequestInit) =>
      new Response(typeof body === "string" ? body : JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
  );

afterEach(() => {
  vi.useRealTimers();
});

describe("compareVersions", () => {
  it("orders releases part by part, as numbers", () => {
    expect(compareVersions("0.1.4", "0.1.3")).toBeGreaterThan(0);
    expect(compareVersions("0.1.3", "0.1.4")).toBeLessThan(0);
    expect(compareVersions("v0.1.3", "0.1.3")).toBe(0);
    expect(compareVersions("0.1.10", "0.1.9")).toBeGreaterThan(0);
    expect(compareVersions("0.2.0", "0.10.0")).toBeLessThan(0);
    expect(compareVersions("1.0.0", "0.99.99")).toBeGreaterThan(0);
  });

  it("puts a prerelease before its release, and ignores build metadata", () => {
    expect(compareVersions("0.2.0-beta.1", "0.2.0")).toBeLessThan(0);
    expect(compareVersions("0.2.0", "0.2.0-beta.1")).toBeGreaterThan(0);
    expect(compareVersions("0.2.0+build.5", "0.2.0")).toBe(0);
  });

  it("has no order for something that isn't a version", () => {
    for (const v of ["latest", "0.1", "v1.2.3.4", "", "1.2.x"])
      expect(compareVersions(v, "0.1.3")).toBeNull();
  });
});

describe("checkForUpdate", () => {
  it("finds a newer release, and links to this repository's own page for it", async () => {
    const fetch = answering(200, {
      tag_name: "v0.1.4",
      html_url: "https://elsewhere.example/v0.1.4",
    });
    expect(await checkForUpdate(fetch, "0.1.3")).toEqual({
      kind: "newer",
      latest: "0.1.4",
      url: "https://github.com/Johnserf-Seed/lanbeam/releases/tag/v0.1.4",
    });
    // One request, for the newest release — and nothing else sent with it.
    expect(fetch).toHaveBeenCalledTimes(1);
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe(
      "https://api.github.com/repos/Johnserf-Seed/lanbeam/releases/latest",
    );
    expect(init).toMatchObject({
      credentials: "omit",
      referrerPolicy: "no-referrer",
    });
  });

  it("says this is the latest when it is — or when this build is newer", async () => {
    expect(
      await checkForUpdate(answering(200, { tag_name: "v0.1.3" }), "0.1.3"),
    ).toEqual({ kind: "current", latest: "0.1.3" });
    expect(
      await checkForUpdate(answering(200, { tag_name: "v0.1.3" }), "0.2.0"),
    ).toEqual({ kind: "current", latest: "0.1.3" });
  });

  it("can't tell when GitHub doesn't say: no release, rate limited, a bad answer", async () => {
    for (const fetch of [
      answering(404, { message: "Not Found" }),
      answering(403, { message: "API rate limit exceeded" }),
      // An error status is never read as an answer, whatever its body says.
      answering(500, { tag_name: "v9.9.9" }),
      answering(200, { name: "no tag here" }),
      answering(200, { tag_name: "nightly" }),
      answering(200, "<html>not json</html>"),
      answering(200, null),
    ])
      expect(await checkForUpdate(fetch, "0.1.3")).toEqual({ kind: "failed" });
  });

  it("can't tell when offline", async () => {
    const offline = vi.fn(async () => {
      throw new TypeError("Failed to fetch");
    });
    expect(await checkForUpdate(offline, "0.1.3")).toEqual({ kind: "failed" });
  });

  it("gives up after ten seconds instead of checking forever", async () => {
    vi.useFakeTimers();
    // Answers only by failing once the check stops waiting.
    const silent = vi.fn(
      (_url: RequestInfo | URL, init?: RequestInit) =>
        new Promise<Response>((_, reject) =>
          init?.signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          ),
        ),
    );
    const result = checkForUpdate(silent, "0.1.3");
    await vi.advanceTimersByTimeAsync(9_999);
    expect(silent.mock.calls[0][1]?.signal?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await result).toEqual({ kind: "failed" });
  });

  it("falls back to this repository's releases page", () => {
    expect(RELEASES_URL).toBe(
      "https://github.com/Johnserf-Seed/lanbeam/releases/latest",
    );
  });
});
