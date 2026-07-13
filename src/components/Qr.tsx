import { useMemo } from "react";
import { create } from "qrcode";

/** A real, scannable QR code rendered as a crisp, self-contained SVG.
 *
 *  `value` is encoded at error-correction level M; every dark module becomes an
 *  integer-coordinate <rect> (adjacent modules on a row are merged into one run
 *  so the DOM stays small) over a white background, framed by a `margin`-module
 *  quiet zone. The viewBox is the module count (plus quiet zone) and the SVG is
 *  scaled to `size` px, so it stays sharp at any size (shapeRendering keeps the
 *  module edges crisp).
 *
 *  CRITICAL: the code is ALWAYS dark-on-white regardless of the app theme —
 *  scanners need the contrast — so the two colors are hard-wired here and must
 *  never be theme vars. Everything is generated inline: no canvas, no images,
 *  no external requests.
 *
 *  An empty or unencodable `value` renders a plain white placeholder of the same
 *  footprint instead of throwing or showing a broken graphic. */
export default function Qr({
  value,
  size,
  margin = 4,
  radius = 10,
}: {
  value: string;
  size: number;
  margin?: number;
  radius?: number;
}) {
  const data = useMemo(() => {
    if (!value) return null;
    try {
      const { modules } = create(value, { errorCorrectionLevel: "M" });
      const count = modules.size;
      // Merge each row's dark modules into horizontal runs — one <rect> per run
      // instead of per module. Runs never overlap and each starts at a distinct
      // (x, y), so `${x}-${y}` is a stable, unique React key.
      const runs: { x: number; y: number; w: number }[] = [];
      for (let r = 0; r < count; r++) {
        let start = -1;
        for (let c = 0; c < count; c++) {
          const dark = modules.get(r, c) === 1;
          if (dark && start < 0) start = c;
          if (!dark && start >= 0) {
            runs.push({ x: margin + start, y: margin + r, w: c - start });
            start = -1;
          }
        }
        if (start >= 0) {
          runs.push({ x: margin + start, y: margin + r, w: count - start });
        }
      }
      return { dim: count + margin * 2, runs };
    } catch {
      // A value too large for a single symbol (or any encoder error) falls back
      // to the placeholder rather than crashing the surrounding modal.
      return null;
    }
  }, [value, margin]);

  if (!data) {
    return (
      <div
        style={{
          width: size,
          height: size,
          borderRadius: radius,
          background: "#ffffff",
          border: "1px solid var(--border2)",
          flex: "none",
        }}
      />
    );
  }

  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${data.dim} ${data.dim}`}
      shapeRendering="crispEdges"
      role="img"
      aria-label="QR"
      style={{ display: "block", borderRadius: radius, flex: "none" }}
    >
      <rect width={data.dim} height={data.dim} fill="#ffffff" />
      {data.runs.map((run) => (
        <rect
          key={`${run.x}-${run.y}`}
          x={run.x}
          y={run.y}
          width={run.w}
          height={1}
          fill="#1a1a1a"
        />
      ))}
    </svg>
  );
}
