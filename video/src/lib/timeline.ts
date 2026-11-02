import type { Pt } from "./anim";

// Scene windows in seconds (global time). Neighbours overlap: the outgoing
// scene hands its light to the incoming one, so there is never a hard cut.
export const T = {
  intro: [0, 4.8],
  discover: [4.2, 10.6],
  send: [10.2, 20.6],
  text: [20.0, 25.8],
  share: [25.2, 31.8],
  pair: [31.2, 37.6],
  trust: [37.0, 42.0],
  look: [41.4, 48.0],
  outro: [47.4, 53.4],
} as const;

export const DURATION = 53.4;

export type Span = readonly [number, number];
export const inSpan = (t: number, s: Span, pad = 0) =>
  t >= s[0] - pad && t <= s[1] + pad;

export const RADAR_C = { x: 1100, y: 560 };
export const RADAR_Z = 2.0;

// Where each scene's main panel sits (screen px).
export const SEND_C = { x: 960, y: 590 };
export const TEXT_C = { x: 660, y: 655 };
export const SHARE_C = { x: 1250, y: 640 };
export const PAIR_C = { x: 720, y: 630 };
export const TRUST_C = { x: 1230, y: 590 };
export const LOOK_C = { x: 1215, y: 560 };

/** A panel folds into a point of light at `fold`; the light leaves at `t0`
 *  and opens the next panel at `t1`. */
export type Handoff = { fold: number; t0: number; t1: number; from: Pt; to: Pt; bend: number };

const hand = (fold: number, from: Pt, to: Pt, bend: number): Handoff => ({
  fold,
  t0: fold + 0.42,
  t1: fold + 1.0,
  from,
  to,
  bend,
});

// The score (scripts/score.mjs) runs at 120 BPM from the moment the radar hub
// lights up (4.78 s), and every panel opens on one of its beats: t1 = 4.78 +
// 0.5·n. The clicks and chimes inside the scenes sit on that grid too. Move a
// time here or in a scene, and move its cue in score.mjs with it.
export const H = {
  sendText: hand(19.78, SEND_C, TEXT_C, -0.35),
  textShare: hand(24.78, TEXT_C, SHARE_C, 0.3),
  sharePair: hand(30.78, SHARE_C, PAIR_C, -0.3),
  pairTrust: hand(36.78, PAIR_C, TRUST_C, 0.32),
  trustLook: hand(41.28, TRUST_C, LOOK_C, -0.4),
  lookOutro: hand(47.28, LOOK_C, { x: 630.7, y: 449.3 }, 0.3),
};
