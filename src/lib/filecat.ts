export type FileCat = "img" | "vid" | "aud" | "arc" | "doc" | "oth";

const IMG = [
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
];
const VID = ["MP4", "MOV", "MKV", "AVI", "WEBM", "M4V"];
const AUD = ["MP3", "WAV", "FLAC", "AAC", "M4A", "OGG"];
const ARC = ["ZIP", "RAR", "7Z", "TAR", "GZ", "DMG", "ISO", "PKG"];
const DOC = [
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
];

export function fileCat(ext: string | undefined): FileCat {
  const e = String(ext || "").toUpperCase();
  if (IMG.includes(e)) return "img";
  if (VID.includes(e)) return "vid";
  if (AUD.includes(e)) return "aud";
  if (ARC.includes(e)) return "arc";
  if (DOC.includes(e)) return "doc";
  return "oth";
}

/** [fg, bg] CSS values for the category chip of a file extension. */
export function catColors(ext: string | undefined): [string, string] {
  const m: Record<FileCat, [string, string]> = {
    img: ["var(--cat-img)", "var(--cat-img-soft)"],
    vid: ["var(--cat-vid)", "var(--cat-vid-soft)"],
    aud: ["var(--cat-aud)", "var(--cat-aud-soft)"],
    doc: ["var(--cat-doc)", "var(--cat-doc-soft)"],
    arc: ["var(--accent-ink)", "var(--accent-soft)"],
    oth: ["var(--muted2)", "var(--sidebar)"],
  };
  return m[fileCat(ext)];
}
