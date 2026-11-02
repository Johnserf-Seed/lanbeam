import { AbsoluteFill } from "remotion";
import { C } from "../lib/theme";

/** Near-black field with a faint dot grid and a light pool that follows the
 *  action (the reference's look, in LanBeam's teal). */
export function Background({
  spot = { x: 960, y: 540 },
  spotPower = 1,
  gridOpacity = 1,
}: {
  spot?: { x: number; y: number };
  spotPower?: number;
  gridOpacity?: number;
}) {
  return (
    <AbsoluteFill style={{ background: C.void }}>
      <AbsoluteFill
        style={{
          background: `radial-gradient(1100px 760px at ${spot.x}px ${spot.y}px, rgba(31, 122, 150, ${0.16 * spotPower}) 0%, rgba(16, 21, 29, ${0.55 * spotPower}) 45%, rgba(4, 7, 10, 0) 75%)`,
        }}
      />
      <AbsoluteFill
        style={{
          opacity: 0.55 * gridOpacity,
          backgroundImage:
            "radial-gradient(circle, rgba(147, 222, 222, 0.16) 1.1px, transparent 1.6px)",
          backgroundSize: "30px 30px",
          backgroundPosition: "15px 15px",
          WebkitMaskImage: `radial-gradient(900px 640px at ${spot.x}px ${spot.y}px, #000 0%, rgba(0,0,0,0.35) 55%, transparent 85%)`,
          maskImage: `radial-gradient(900px 640px at ${spot.x}px ${spot.y}px, #000 0%, rgba(0,0,0,0.35) 55%, transparent 85%)`,
        }}
      />
      <AbsoluteFill
        style={{
          background:
            "radial-gradient(ellipse at center, transparent 55%, rgba(0, 0, 0, 0.55) 100%)",
        }}
      />
    </AbsoluteFill>
  );
}
