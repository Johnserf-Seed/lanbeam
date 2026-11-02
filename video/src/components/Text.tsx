import type { CSSProperties, ReactNode } from "react";
import { E, lerp, prog } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";

/** Rise + unblur on the way in, lift + blur on the way out. */
export function reveal(
  t: number,
  at: number,
  out = Infinity,
  { dy = 22, blur = 12, dur = 0.75 } = {},
): CSSProperties {
  const p = prog(t, at, at + dur, E.out);
  const q = prog(t, out, out + 0.45, E.in);
  // `translate`, not `transform`: callers keep their own transform (centring).
  return {
    opacity: p * (1 - q),
    translate: `0 ${lerp(dy, 0, p) - q * dy * 0.6}px`,
    filter: `blur(${lerp(blur, 0, p) + q * blur * 0.7}px)`,
  };
}

/** Text that arrives one character at a time. */
export function CharReveal({
  text,
  t,
  at,
  out = Infinity,
  stagger = 0.035,
  style,
}: {
  text: string;
  t: number;
  at: number;
  out?: number;
  stagger?: number;
  style?: CSSProperties;
}) {
  return (
    <span style={{ display: "inline-block", whiteSpace: "pre", ...style }}>
      {Array.from(text).map((ch, i) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: static text
          key={i}
          style={{
            display: "inline-block",
            ...reveal(t, at + i * stagger, out, { dy: 26, blur: 14, dur: 0.7 }),
          }}
        >
          {ch === " " ? " " : ch}
        </span>
      ))}
    </span>
  );
}

/** The scene caption: a numbered label, a headline and one line of detail. */
export function Caption({
  t,
  at,
  out,
  index,
  label,
  title,
  sub,
  x,
  y,
  align = "left",
  width = 640,
}: {
  t: number;
  at: number;
  out: number;
  index: string;
  label: string;
  title: string;
  sub?: ReactNode;
  x: number;
  y: number;
  align?: "left" | "center";
  width?: number;
}) {
  return (
    <div
      style={{
        position: "absolute",
        left: align === "center" ? x - width / 2 : x,
        top: y,
        width,
        textAlign: align,
        fontFamily: FONT,
        color: C.ink2,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 14,
          justifyContent: align === "center" ? "center" : "flex-start",
          ...reveal(t, at, out, { dy: 14, blur: 8 }),
        }}
      >
        <span
          style={{
            fontFamily: MONO,
            fontSize: 21,
            fontWeight: 600,
            color: C.accent,
            letterSpacing: "0.08em",
          }}
        >
          {index}
        </span>
        <span
          style={{
            width: lerp(0, 34, prog(t, at + 0.1, at + 0.8, E.out)),
            height: 1.5,
            background: C.accent,
            opacity: 0.55,
          }}
        />
        <span
          style={{
            fontSize: 23,
            fontWeight: 500,
            color: C.muted2,
            letterSpacing: "0.16em",
          }}
        >
          {label}
        </span>
      </div>
      <div
        style={{
          marginTop: 18,
          fontSize: 68,
          lineHeight: 1.15,
          fontWeight: 600,
          letterSpacing: "0.02em",
          color: "#eef4f8",
          textShadow: "0 0 40px rgba(127, 212, 212, 0.18)",
        }}
      >
        <CharReveal text={title} t={t} at={at + 0.12} out={out} />
      </div>
      {sub != null && (
        <div
          style={{
            marginTop: 20,
            fontSize: 26,
            lineHeight: 1.6,
            color: C.muted2,
            ...reveal(t, at + 0.45, out, { dy: 16, blur: 8 }),
          }}
        >
          {sub}
        </div>
      )}
    </div>
  );
}

/** Text that swaps in place: the old line rolls up and away, the new one rolls
 *  in from below. `steps[i].at` is when step i starts arriving. */
export function RollText({
  t,
  steps,
  height,
  style,
  dur = 0.6,
}: {
  t: number;
  steps: { at: number; text: ReactNode }[];
  height: number;
  style?: CSSProperties;
  dur?: number;
}) {
  // An invisible copy of the current line holds the width, so the text after
  // this span never slides under it while the lines swap.
  let cur = 0;
  steps.forEach((s, i) => {
    if (t >= s.at) cur = i;
  });
  return (
    <span
      style={{
        position: "relative",
        display: "inline-block",
        height,
        verticalAlign: "bottom",
        ...style,
      }}
    >
      <span style={{ visibility: "hidden", whiteSpace: "nowrap" }}>{steps[cur].text}</span>
      {steps.map((s, i) => {
        const next = steps[i + 1];
        const lag = i === 0 ? 0 : dur * 0.3;
        const pin = prog(t, s.at + lag, s.at + lag + dur, E.out);
        const pout = next ? prog(t, next.at, next.at + dur * 0.5, E.in) : 0;
        if (pin <= 0 || pout >= 1) return null;
        const y = (1 - pin) * height * 0.8 - pout * height * 0.7;
        return (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: steps are static
            key={i}
            style={{
              position: "absolute",
              left: 0,
              top: 0,
              whiteSpace: "nowrap",
              display: "inline-block",
              transform: `translateY(${y}px)`,
              opacity: pin * (1 - pout),
              filter: `blur(${(1 - pin) * 8 + pout * 8}px)`,
            }}
          >
            {s.text}
          </span>
        );
      })}
    </span>
  );
}

/** Slot-machine digits: each digit spins down onto its value. */
export function RollDigits({
  value,
  t,
  at,
  dur = 0.9,
  stagger = 0.08,
  spins = 2,
  lineHeight,
  style,
}: {
  value: string;
  t: number;
  at: number;
  dur?: number;
  stagger?: number;
  spins?: number;
  lineHeight: number;
  style?: CSSProperties;
}) {
  let di = 0;
  return (
    <span style={{ display: "inline-flex", whiteSpace: "pre", ...style }}>
      {Array.from(value).map((ch, i) => {
        if (!/[0-9]/.test(ch))
          return (
            // biome-ignore lint/suspicious/noArrayIndexKey: static text
            <span key={i} style={{ opacity: prog(t, at, at + 0.3) }}>
              {ch}
            </span>
          );
        const k = di++;
        const target = Number(ch);
        const p = prog(t, at + k * stagger, at + k * stagger + dur, E.out);
        const v = target + 10 * (spins + (k % 2)) * (1 - p);
        const base = Math.floor(v);
        const frac = v - base;
        const speed = (1 - p) * 10;
        return (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: static text
            key={i}
            style={{
              position: "relative",
              display: "inline-block",
              height: lineHeight,
              lineHeight: `${lineHeight}px`,
              overflow: "hidden",
              opacity: prog(t, at + k * stagger, at + k * stagger + 0.2),
            }}
          >
            <span style={{ visibility: "hidden" }}>0</span>
            {[0, 1].map((j) => (
              <span
                key={j}
                style={{
                  position: "absolute",
                  left: 0,
                  top: 0,
                  transform: `translateY(${(frac - j) * lineHeight}px)`,
                  filter: speed > 0.6 ? `blur(${Math.min(2.4, speed * 0.3)}px)` : undefined,
                }}
              >
                {(((base + j) % 10) + 10) % 10}
              </span>
            ))}
          </span>
        );
      })}
    </span>
  );
}
