import { describe, expect, it } from "vitest";
import { catColors, fileCat } from "./filecat";

describe("fileCat", () => {
  it("classifies image extensions as img", () => {
    for (const ext of [
      "HEIC",
      "JPG",
      "JPEG",
      "PNG",
      "GIF",
      "WEBP",
      "RAW",
      "DNG",
      "BMP",
      "SVG",
      "TIFF",
    ]) {
      expect(fileCat(ext)).toBe("img");
    }
  });

  it("classifies video extensions as vid", () => {
    for (const ext of ["MP4", "MOV", "MKV", "AVI", "WEBM", "M4V"]) {
      expect(fileCat(ext)).toBe("vid");
    }
  });

  it("classifies audio extensions as aud", () => {
    for (const ext of ["MP3", "WAV", "FLAC", "AAC", "M4A", "OGG"]) {
      expect(fileCat(ext)).toBe("aud");
    }
  });

  it("classifies archive extensions as arc", () => {
    for (const ext of ["ZIP", "RAR", "7Z", "TAR", "GZ", "DMG", "ISO", "PKG"]) {
      expect(fileCat(ext)).toBe("arc");
    }
  });

  it("classifies document extensions as doc", () => {
    for (const ext of [
      "PDF",
      "DOC",
      "DOCX",
      "KEY",
      "PPT",
      "PPTX",
      "XLS",
      "XLSX",
      "TXT",
      "MD",
      "PAGE",
      "NUMB",
      "CSV",
    ]) {
      expect(fileCat(ext)).toBe("doc");
    }
  });

  it("returns oth for unknown extensions", () => {
    expect(fileCat("EXE")).toBe("oth");
    expect(fileCat("XYZ")).toBe("oth");
    expect(fileCat("bin")).toBe("oth");
  });

  it("is case-insensitive", () => {
    expect(fileCat("jpg")).toBe("img");
    expect(fileCat("Mp4")).toBe("vid");
    expect(fileCat("pDf")).toBe("doc");
  });

  it("returns oth for undefined, empty, and whitespace-ish input", () => {
    expect(fileCat(undefined)).toBe("oth");
    expect(fileCat("")).toBe("oth");
    expect(fileCat("  ")).toBe("oth");
  });
});

describe("catColors", () => {
  it("returns the mapped [fg, bg] tuple for each category", () => {
    expect(catColors("PNG")).toEqual(["var(--cat-img)", "var(--cat-img-soft)"]);
    expect(catColors("MP4")).toEqual(["var(--cat-vid)", "var(--cat-vid-soft)"]);
    expect(catColors("MP3")).toEqual(["var(--cat-aud)", "var(--cat-aud-soft)"]);
    expect(catColors("PDF")).toEqual(["var(--cat-doc)", "var(--cat-doc-soft)"]);
    expect(catColors("ZIP")).toEqual([
      "var(--accent-ink)",
      "var(--accent-soft)",
    ]);
  });

  it("returns the default tuple for unknown extensions", () => {
    expect(catColors("EXE")).toEqual(["var(--muted2)", "var(--sidebar)"]);
    expect(catColors(undefined)).toEqual(["var(--muted2)", "var(--sidebar)"]);
    expect(catColors("")).toEqual(["var(--muted2)", "var(--sidebar)"]);
  });

  it("returns a two-element tuple of strings", () => {
    const result = catColors("JPG");
    expect(result).toHaveLength(2);
    expect(typeof result[0]).toBe("string");
    expect(typeof result[1]).toBe("string");
  });
});
