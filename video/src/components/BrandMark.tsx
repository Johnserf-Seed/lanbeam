import { useId } from "react";
import { C } from "../lib/theme";

/** The LanBeam mark (assets/brand/app-icon.svg) with each part animatable:
 *  the squircle, the source dot, and the three arcs drawing outward. */
export function BrandMark({
  size,
  square = 1,
  dot = 1,
  arcs = [1, 1, 1],
  glow = 1,
}: {
  size: number;
  /** squircle 0..1 (scale + fade) */
  square?: number;
  /** dot scale */
  dot?: number;
  /** arc draw progress, inner → outer */
  arcs?: [number, number, number];
  glow?: number;
}) {
  const gid = useId().replace(/:/g, "");
  // Arc lengths: quarter circles of radius 24, 40, 56.
  const lens = [24, 40, 56].map((r) => (Math.PI * r) / 2);
  const paths = [
    "M26 46A24 24 0 0 1 50 70",
    "M26 30A40 40 0 0 1 66 70",
    "M26 14A56 56 0 0 1 82 70",
  ];
  const op = [1, 0.58, 0.3];
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 96 96"
      style={{ overflow: "visible", display: "block" }}
    >
      <defs>
        <linearGradient id={`g${gid}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor={C.brandA} />
          <stop offset="1" stopColor={C.brandB} />
        </linearGradient>
        <radialGradient id={`r${gid}`}>
          <stop offset="0" stopColor={C.brandA} stopOpacity={0.5} />
          <stop offset="1" stopColor={C.brandA} stopOpacity={0} />
        </radialGradient>
      </defs>
      <circle
        cx={48}
        cy={52}
        r={70}
        fill={`url(#r${gid})`}
        opacity={0.55 * glow * square}
      />
      <g
        style={{
          transformOrigin: "48px 48px",
          transform: `scale(${0.72 + 0.28 * square})`,
          opacity: Math.min(1, square * 1.4),
        }}
      >
        <rect x={2} y={2} width={92} height={92} rx={21} fill={`url(#g${gid})`} />
        <rect
          x={2.5}
          y={2.5}
          width={91}
          height={91}
          rx={20.5}
          fill="none"
          stroke="rgba(255,255,255,0.22)"
          strokeWidth={1}
        />
      </g>
      <g transform="translate(48 48) scale(.64) translate(-48 -48)">
        <circle
          cx={26}
          cy={70}
          r={9.5}
          fill="#fff"
          style={{
            transformOrigin: "26px 70px",
            transform: `scale(${dot})`,
          }}
        />
        {paths.map((d, i) => (
          <path
            key={d}
            d={d}
            stroke="#fff"
            strokeWidth={10}
            fill="none"
            strokeLinecap="round"
            opacity={arcs[i] > 0.001 ? op[i] : 0}
            strokeDasharray={`${lens[i]} ${lens[i] + 20}`}
            strokeDashoffset={lens[i] * (1 - arcs[i])}
          />
        ))}
      </g>
    </svg>
  );
}

/** Where the mark's source dot sits, as a fraction of its size. */
export const MARK_DOT = { x: 33.92 / 96, y: 62.08 / 96 };
