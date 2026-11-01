// Checking for a newer LanBeam (F2). Only when someone presses 检查更新 — the
// app opens no other connection off the LAN — and all it asks GitHub for is
// the newest release's tag. Nothing is downloaded or installed: a newer version
// gets a link to its release page, which opens in the browser.
import * as api from "../bridge/api";
import { APP_VERSION } from "./version";

/** The repository releases are published from (tauri.conf.json's homepage). */
const REPO = "Johnserf-Seed/lanbeam";
/** The newest release's page, for a check that came back without an answer. */
export const RELEASES_URL = `https://github.com/${REPO}/releases/latest`;
/** GitHub's newest published release — drafts and prereleases left out. */
const LATEST_API = `https://api.github.com/repos/${REPO}/releases/latest`;
/** A check gives up after this, so the button can't sit on 正在检查 for good. */
const CHECK_TIMEOUT_MS = 10_000;

export type UpdateCheck =
  /** `latest` is newer than this build; `url` is its release page. */
  | { kind: "newer"; latest: string; url: string }
  /** This build is the newest release (or newer than any, like a dev build). */
  | { kind: "current"; latest: string }
  /** No answer worth showing: offline, rate-limited, no release yet, or a tag
   *  that isn't a version. */
  | { kind: "failed" };

/** A release version's parts: `v1.2.3` or `1.2.3`, maybe `-pre` or `+build`. */
function parse(v: string): { nums: number[]; pre: boolean } | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)([-+].*)?$/.exec(v.trim());
  if (!m) return null;
  return {
    nums: [m[1], m[2], m[3]].map(Number),
    pre: m[4]?.startsWith("-") ?? false,
  };
}

/** Order two release versions: negative when `a` is older than `b`, 0 for the
 *  same release, positive when newer — or `null` when either isn't a version.
 *  Parts compare as numbers (0.1.10 comes after 0.1.9), and a prerelease comes
 *  before its release (0.2.0-beta.1 before 0.2.0). */
export function compareVersions(a: string, b: string): number | null {
  const pa = parse(a);
  const pb = parse(b);
  if (!pa || !pb) return null;
  for (let i = 0; i < 3; i++) {
    if (pa.nums[i] !== pb.nums[i]) return pa.nums[i] - pb.nums[i];
  }
  return Number(pb.pre) - Number(pa.pre);
}

/** Ask GitHub for the newest release and hold it against this build. Never
 *  throws: every way it can go wrong is a `failed` the UI can say plainly.
 *  (`fetchImpl` and `current` are there for the tests.) */
export async function checkForUpdate(
  fetchImpl: typeof fetch = (input, init) => fetch(input, init),
  current: string = APP_VERSION,
): Promise<UpdateCheck> {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), CHECK_TIMEOUT_MS);
  try {
    const res = await fetchImpl(LATEST_API, {
      headers: { Accept: "application/vnd.github+json" },
      // The version is all it asks for: no cookies, no referrer, no cache.
      credentials: "omit",
      referrerPolicy: "no-referrer",
      cache: "no-store",
      signal: ctl.signal,
    });
    if (!res.ok) return { kind: "failed" };
    const body: unknown = await res.json();
    const raw = (body as { tag_name?: unknown } | null)?.tag_name;
    const tag = typeof raw === "string" ? raw.trim() : "";
    const order = compareVersions(tag, current);
    if (order === null) return { kind: "failed" };
    const latest = tag.replace(/^v/, "");
    if (order <= 0) return { kind: "current", latest };
    // The page is built from the tag, never taken from the response: what
    // opens in the browser is always this repository's own release page.
    const url = `https://github.com/${REPO}/releases/tag/${encodeURIComponent(tag)}`;
    return { kind: "newer", latest, url };
  } catch {
    return { kind: "failed" };
  } finally {
    clearTimeout(timer);
  }
}

/** Open a release page in the default browser (a new tab in browser mode). */
export async function openReleasePage(url: string): Promise<void> {
  if (!api.isTauri) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}
