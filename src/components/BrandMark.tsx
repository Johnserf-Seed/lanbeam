import { useId } from "react";

/** The LanBeam mark — the radar/beacon squircle from the brand kit
 *  (`Lanbeam工具logo设计/export/icon/app-icon.svg`). Inlined rather than loaded
 *  as a file so it needs no network, never flashes, and stays crisp at any size.
 *  The squircle is PART of the artwork, so callers need no border-radius of
 *  their own — just give it a size. */
export default function BrandMark({ size = 30 }: { size?: number }) {
  // A gradient id must be unique per document: two marks on one page would
  // otherwise declare the same <linearGradient> and fight over it.
  const gid = useId();
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 96 96"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      // Let clicks/drags fall through to the parent (the sidebar's title bar is
      // a Tauri drag region — the mark must not swallow the grab).
      style={{ flex: "none", display: "block", pointerEvents: "none" }}
    >
      <defs>
        <linearGradient id={gid} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#35ABC9" />
          <stop offset="1" stopColor="#1F7A96" />
        </linearGradient>
      </defs>
      <rect x="2" y="2" width="92" height="92" rx="21" fill={`url(#${gid})`} />
      {/* The beacon: a source dot plus three arcs radiating out, fading. */}
      <g transform="translate(48 48) scale(.64) translate(-48 -48)">
        <circle cx="26" cy="70" r="9.5" fill="#fff" />
        <path
          d="M26 46A24 24 0 0 1 50 70"
          stroke="#fff"
          strokeWidth="10"
          fill="none"
          strokeLinecap="round"
        />
        <path
          d="M26 30A40 40 0 0 1 66 70"
          stroke="#fff"
          strokeWidth="10"
          fill="none"
          strokeLinecap="round"
          opacity=".58"
        />
        <path
          d="M26 14A56 56 0 0 1 82 70"
          stroke="#fff"
          strokeWidth="10"
          fill="none"
          strokeLinecap="round"
          opacity=".3"
        />
      </g>
    </svg>
  );
}
