// Unit tests for the pure formatters in ./format. These are deterministic:
// byte/size math, path helpers, and time bucketing all take explicit inputs
// (and an explicit `now`), so no timers or IPC are involved.
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import { baseName, extOf, fmtBytes, fmtWhen, whenGroup } from "./format";

const MB = 1048576;

describe("fmtBytes", () => {
  it("returns 0 KB for zero and negative inputs", () => {
    expect(fmtBytes(0)).toBe("0 KB");
    expect(fmtBytes(-5)).toBe("0 KB");
  });

  it("formats sub-megabyte sizes in whole KB", () => {
    // 0.5 MB -> 0.5 * 1024 = 512 KB
    expect(fmtBytes(MB * 0.5)).toBe("512 KB");
  });

  it("clamps a single byte up to at least 1 KB", () => {
    expect(fmtBytes(1)).toBe("1 KB");
  });

  it("uses one decimal for the 1..10 MB branch", () => {
    expect(fmtBytes(MB)).toBe("1.0 MB");
    expect(fmtBytes(MB * 2.5)).toBe("2.5 MB");
  });

  it("rounds to whole MB at 10 MB and above", () => {
    expect(fmtBytes(MB * 10)).toBe("10 MB");
    expect(fmtBytes(MB * 12.7)).toBe("13 MB");
  });

  it("switches to GB once the size reaches 1000 MB", () => {
    // 1000 MB -> 1000 / 1024 = 0.9766 -> "1.0 GB"
    expect(fmtBytes(MB * 1000)).toBe("1.0 GB");
  });

  it("formats a huge size in GB with one decimal", () => {
    // 2048 MB -> 2048 / 1024 = 2.0 GB
    expect(fmtBytes(MB * 2048)).toBe("2.0 GB");
  });
});

describe("whenGroup", () => {
  const now = new Date(2026, 6, 13, 12, 0, 0).getTime();

  it("buckets earlier-the-same-day as today", () => {
    const ts = new Date(2026, 6, 13, 1, 0, 0).getTime();
    expect(whenGroup(ts, now)).toBe("today");
  });

  it("buckets a later-the-same-day (or future) timestamp as today", () => {
    const laterToday = new Date(2026, 6, 13, 23, 0, 0).getTime();
    const future = new Date(2026, 6, 20, 8, 0, 0).getTime();
    expect(whenGroup(laterToday, now)).toBe("today");
    expect(whenGroup(future, now)).toBe("today");
  });

  it("buckets the previous calendar day as yday", () => {
    const ts = new Date(2026, 6, 12, 23, 30, 0).getTime();
    expect(whenGroup(ts, now)).toBe("yday");
  });

  it("buckets two-or-more days back as earlier", () => {
    const ts = new Date(2026, 6, 11, 12, 0, 0).getTime();
    expect(whenGroup(ts, now)).toBe("earlier");
  });
});

describe("fmtWhen", () => {
  const now = new Date(2026, 6, 13, 12, 0, 0).getTime();

  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows a zero-padded HH:MM clock for today", () => {
    const ts = new Date(2026, 6, 13, 9, 5, 0).getTime();
    expect(fmtWhen(ts, now)).toBe("09:05");
  });

  it("uses the localized yesterday string for yday (en)", () => {
    const ts = new Date(2026, 6, 12, 18, 5, 0).getTime();
    expect(fmtWhen(ts, now)).toBe("Yesterday 18:05");
  });

  it("uses the localized yesterday string for yday (zh)", async () => {
    await i18n.changeLanguage("zh");
    const ts = new Date(2026, 6, 12, 18, 5, 0).getTime();
    expect(fmtWhen(ts, now)).toBe("昨天 18:05");
  });

  it("uses a short intl date for earlier timestamps (en)", () => {
    const ts = new Date(2026, 6, 6, 15, 0, 0).getTime();
    const expected = new Intl.DateTimeFormat("en", {
      month: "short",
      day: "numeric",
    }).format(new Date(ts));
    expect(fmtWhen(ts, now)).toBe(expected);
  });

  it("uses the M月D日 form for earlier timestamps (zh)", async () => {
    await i18n.changeLanguage("zh");
    const ts = new Date(2026, 6, 6, 15, 0, 0).getTime();
    expect(fmtWhen(ts, now)).toBe("7月6日");
  });
});

describe("baseName", () => {
  it("extracts the file name from a posix path", () => {
    expect(baseName("/a/b/c.txt")).toBe("c.txt");
  });

  it("extracts the file name from a windows path", () => {
    expect(baseName("C:\\Users\\x\\file.png")).toBe("file.png");
  });

  it("uses the last separator when both kinds appear", () => {
    expect(baseName("a/b\\c.txt")).toBe("c.txt");
  });

  it("returns the input unchanged when there is no separator", () => {
    expect(baseName("plain.txt")).toBe("plain.txt");
  });
});

describe("extOf", () => {
  it("returns the uppercased extension", () => {
    expect(extOf("file.txt")).toBe("TXT");
    expect(extOf("file.PdF")).toBe("PDF");
  });

  it("uses only the final extension of a multi-dot name", () => {
    expect(extOf("archive.tar.gz")).toBe("GZ");
  });

  it("truncates long extensions to 4 characters", () => {
    expect(extOf("verylong.pictures")).toBe("PICT");
  });

  it("falls back to FILE when there is no usable extension", () => {
    expect(extOf("noext")).toBe("FILE");
    expect(extOf(".hidden")).toBe("FILE");
  });
});
