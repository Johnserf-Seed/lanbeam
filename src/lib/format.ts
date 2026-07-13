import i18n from "../i18n";

/** Size formatting follows the design spec: GB ≥ 1000 MB, whole MB ≥ 10. */
export function fmtMb(mb: number): string {
  if (!mb || mb <= 0) return "0 KB";
  if (mb >= 1000) return `${(mb / 1024).toFixed(1)} GB`;
  if (mb >= 1) return `${mb >= 10 ? Math.round(mb) : mb.toFixed(1)} MB`;
  return `${Math.max(1, Math.round(mb * 1024))} KB`;
}

export function fmtBytes(bytes: number): string {
  return fmtMb(bytes / 1048576);
}

/** SAS codes render as spaced groups of three digits: `483 · 921 · 067`. */
export function fmtSas(v: string | undefined | null): string {
  if (!v) return "";
  const groups: string[] = [];
  for (let i = 0; i < v.length; i += 3) groups.push(v.slice(i, i + 3));
  return groups.join(" · ");
}

/** `m:ss` clock of remaining time — callers wrap it in a translated 剩余 string. */
export function etaClock(
  totalBytes: number,
  percent: number,
  speedBps: number,
): string {
  if (speedBps <= 0) return "";
  const remain = (totalBytes * (100 - percent)) / 100;
  const sec = Math.max(1, Math.round(remain / speedBps));
  return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}`;
}

/** Trailing segment of a `客厅 · Mac mini` style name, ellipsized to 7 chars. */
export function shortName(nm: string): string {
  const parts = String(nm).split("·");
  const t = (parts[parts.length - 1] || nm).trim();
  return t.length > 7 ? `${t.slice(0, 7)}…` : t;
}

/** File name from an absolute path (either separator). */
export function baseName(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i >= 0 ? p.slice(i + 1) : p;
}

/** Uppercased extension, ≤4 chars, for the ext chip. */
export function extOf(name: string): string {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1) : "FILE";
  return ext.toUpperCase().slice(0, 4);
}

/** Estimated rendered width of a radar/trust chip (CJK vs latin metrics). */
export function estChipW(name: string, sub: string): number {
  const tw = (str: string, cj: number, lat: number) => {
    let t = 0;
    for (const ch of String(str)) t += ch.charCodeAt(0) > 0x2e80 ? cj : lat;
    return t;
  };
  return Math.min(214, 44 + Math.max(tw(name, 12.5, 7.2), tw(sub, 10.5, 6.4)));
}

/** `10:24` / `昨天 18:05` / `7月6日` style timestamps, localized to the
 *  active i18n language (zh keeps the M月D日 form; others get a short date). */
export function fmtWhen(ts: number, now = Date.now()): string {
  const d = new Date(ts);
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes(),
  ).padStart(2, "0")}`;
  const g = whenGroup(ts, now);
  if (g === "today") return hm;
  if (g === "yday") return i18n.t("when.yday", { hm });
  return i18n.language?.startsWith("zh")
    ? `${d.getMonth() + 1}月${d.getDate()}日`
    : new Intl.DateTimeFormat(i18n.language, {
        month: "short",
        day: "numeric",
      }).format(d);
}

/** Inbox grouping bucket for a timestamp. */
export function whenGroup(
  ts: number,
  now = Date.now(),
): "today" | "yday" | "earlier" {
  const day = (t: number) => {
    const x = new Date(t);
    return new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  };
  // Round: DST shifts make consecutive local midnights 23 h/25 h apart.
  const diff = Math.round((day(now) - day(ts)) / 86400000);
  if (diff <= 0) return "today";
  if (diff === 1) return "yday";
  return "earlier";
}
