#!/usr/bin/env node
// The promo's soundtrack, synthesized from nothing: no samples, no loops, no
// third-party audio. There is no licence to track — the music is this file.
//
//   node scripts/score.mjs                  # → public/audio/score.wav (48 kHz stereo)
//   SCORE_STATS=1 node scripts/score.mjs    # also print each bus's level and
//                                           # how hard the limiter worked
//
// 120 BPM in D major. The body cycles IV–V–vi–I (G · A · Bm · D/F#) under a
// soft pad, plucked arpeggios, sub bass and light drums; the intro and the
// ending rest on Dmaj9. On top sit the picture's sounds: the light's flights,
// clicks, the codes matching, a transfer finishing, a download, a device
// trusted. Beat 0 is the radar hub lighting up (4.78 s), and every cue below
// mirrors a time in src/lib/timeline.ts or a scene file — move them together.
// Deterministic: a fixed seed, so every run writes the same file.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "public", "audio", "score.wav");
const SR = 48000;
const DUR = 53.4;
const N = Math.round(DUR * SR);

/* ── grid ─────────────────────────────────────────────────────────────── */

const BEAT = 0.5;
const O = 4.78;
const beat = (n) => O + n * BEAT;
const bar = (k) => beat(4 * k);
const STEP = BEAT / 4;
const FINAL = 48.28; // the light lands on the outro mark (beat 87)
const STOP = 47.28; // the window folds away; the groove stops (beat 85)

/* ── utilities ────────────────────────────────────────────────────────── */

let seed = 20261102;
function rnd() {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const noise = () => rnd() * 2 - 1;
const mtof = (m) => 440 * 2 ** ((m - 69) / 12);
const clamp = (x, a, b) => Math.min(b, Math.max(a, x));
const smooth = (x) => {
  const c = clamp(x, 0, 1);
  return c * c * (3 - 2 * c);
};
const S = (t) => Math.round(t * SR);
/** Equal-power pan, -1 (left) … 1 (right). */
function pan(p) {
  const a = ((clamp(p, -1, 1) + 1) * Math.PI) / 4;
  return [Math.cos(a), Math.sin(a)];
}
/** Screen x (0…1920) → a gentle pan. */
const xpan = (x) => ((x / 1920) * 2 - 1) * 0.7;

const TL = 4096;
function table(harmonics) {
  const t = new Float64Array(TL + 1);
  let peak = 0;
  for (let i = 0; i <= TL; i++) {
    let v = 0;
    for (let k = 0; k < harmonics.length; k++) v += harmonics[k] * Math.sin((2 * Math.PI * (k + 1) * i) / TL);
    t[i] = v;
    peak = Math.max(peak, Math.abs(v));
  }
  for (let i = 0; i <= TL; i++) t[i] /= peak;
  return t;
}
const SINE = table([1]);
// A soft saw for the pad: its harmonics fall off faster than a saw's.
const PADW = table(Array.from({ length: 14 }, (_, i) => 1 / (i + 1) ** 1.55));
function look(tbl, ph) {
  const x = (ph - Math.floor(ph)) * TL;
  const i = x | 0;
  return tbl[i] + (tbl[i + 1] - tbl[i]) * (x - i);
}

/** Cubic-bezier easing, as in CSS / Remotion's Easing.bezier. */
function bezier(x1, y1, x2, y2) {
  const cx = 3 * x1, bx = 3 * (x2 - x1) - cx, ax = 1 - cx - bx;
  const cy = 3 * y1, by = 3 * (y2 - y1) - cy, ay = 1 - cy - by;
  const X = (u) => ((ax * u + bx) * u + cx) * u;
  const Y = (u) => ((ay * u + by) * u + cy) * u;
  return (x) => {
    if (x <= 0) return 0;
    if (x >= 1) return 1;
    let lo = 0, hi = 1, u = x;
    for (let i = 0; i < 40; i++) {
      u = (lo + hi) / 2;
      if (X(u) < x) lo = u;
      else hi = u;
    }
    return Y(u);
  };
}
const easeOut = bezier(0.16, 1, 0.3, 1); // src/lib/anim.ts E.out

/* ── buses ────────────────────────────────────────────────────────────── */

const bus = () => [new Float32Array(N), new Float32Array(N)];
const PAD = bus();
const BASS = bus();
const ARP = bus();
const BELL = bus();
const DRUM = bus();
const SFX = bus();
/** Sidechain: the pad and bass breathe under the kick. 1 = untouched. */
const DUCK = new Float32Array(N).fill(1);

/* ── harmony ──────────────────────────────────────────────────────────── */

const CH = {
  D: { bass: 38, pad: [61, 64, 66, 69], arp: [62, 66, 69, 73, 74, 76, 78, 81] }, // Dmaj9
  G: { bass: 43, pad: [59, 62, 66, 69], arp: [67, 69, 71, 74, 78, 79, 81, 83] }, // Gmaj9
  A: { bass: 45, pad: [61, 64, 69, 71], arp: [64, 69, 71, 73, 76, 81, 83, 85] }, // Aadd9
  Bm: { bass: 47, pad: [62, 64, 66, 71], arp: [66, 69, 71, 74, 76, 78, 81, 83] }, // Bm(add11)
  DF: { bass: 42, pad: [57, 62, 64, 69], arp: [64, 66, 69, 74, 76, 78, 81, 86] }, // D/F#
};
const PROG = [CH.G, CH.A, CH.Bm, CH.DF];
const chordAt = (t) => (t < O || t >= FINAL ? CH.D : PROG[Math.floor((t - O) / (4 * BEAT)) % 4]);

/* ── instruments ──────────────────────────────────────────────────────── */

function padNote(t0, t1, midi, amp, p, attack, release) {
  const f = mtof(midi);
  const det = [-0.0045, 0, 0.0042]; // about ±7.5 cents
  const ph = [rnd(), rnd(), rnd()];
  const inc = det.map((d) => (f * (1 + d)) / SR);
  const [gl, gr] = pan(p);
  const s0 = Math.max(0, S(t0));
  const s1 = Math.min(N, S(t1 + release * 1.6));
  const relK = Math.exp(-1 / ((release / 4.5) * SR));
  let rel = 1;
  for (let s = s0; s < s1; s++) {
    const t = s / SR;
    let env = smooth((t - t0) / attack);
    if (t > t1) {
      rel *= relK;
      env *= rel;
    }
    const a = look(PADW, ph[0]);
    const b = look(PADW, ph[1]);
    const c = look(PADW, ph[2]);
    ph[0] += inc[0];
    ph[1] += inc[1];
    ph[2] += inc[2];
    const e = env * amp;
    PAD[0][s] += e * gl * (0.9 * a + 0.6 * b + 0.35 * c);
    PAD[1][s] += e * gr * (0.35 * a + 0.6 * b + 0.9 * c);
  }
}
function padChord(t0, t1, notes, amp = 1, attack = 0.4, release = 1.1) {
  notes.forEach((m, i) => padNote(t0, t1, m, 0.056 * amp, (i / (notes.length - 1)) * 0.9 - 0.45, attack, release));
}

function bassNote(t0, len, midi, amp, attack = 0.006, release = 0.09) {
  const f = mtof(midi);
  const s0 = Math.max(0, S(t0));
  const s1 = Math.min(N, S(t0 + len + release * 6));
  let ph = 0;
  for (let s = s0; s < s1; s++) {
    const t = s / SR - t0;
    let env = clamp(t / attack, 0, 1);
    if (t > len) env *= Math.exp(-(t - len) / release);
    const v = look(SINE, ph) + 0.25 * look(SINE, 2 * ph) + 0.08 * look(SINE, 3 * ph) + 0.15 * look(SINE, ph / 2);
    ph += f / SR;
    const x = v * env * amp;
    BASS[0][s] += x;
    BASS[1][s] += x;
  }
}

/** A plucked, slightly glassy tone: decaying partials, the high ones faster. */
function pluck(t0, midi, vel, p, out, { decay = 0.42, parts = 5, inharm = 0.0005, bright = 1 } = {}) {
  const f = mtof(midi);
  const [gl, gr] = pan(p);
  const s0 = S(t0);
  const len = Math.min(N - s0, S(decay * 5));
  const P = [];
  for (let k = 1; k <= parts; k++) {
    const fk = f * k * (1 + inharm * k * k);
    if (fk > 15000) break;
    P.push({ inc: fk / SR, ph: 0, a: ((k === 1 ? 1 : bright) / k ** 1.5) * vel, d: Math.exp(-1 / ((decay / k ** 0.65) * SR)) });
  }
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    if (s < 0) continue;
    let v = 0;
    for (const q of P) {
      v += q.a * look(SINE, q.ph);
      q.ph += q.inc;
      q.a *= q.d;
    }
    v *= Math.min(1, i / (0.003 * SR));
    out[0][s] += v * gl;
    out[1][s] += v * gr;
  }
}

/** A bell: inharmonic partials with long, staggered decays. */
function bell(t0, midi, vel, p = 0) {
  const ratios = [1, 2.0, 2.76, 4.07, 5.43];
  const amps = [1, 0.42, 0.22, 0.14, 0.08];
  const taus = [1.9, 1.0, 0.65, 0.4, 0.25];
  const f = mtof(midi);
  const [gl, gr] = pan(p);
  const s0 = S(t0);
  const len = Math.min(N - s0, S(2.6));
  const P = ratios
    .map((r, k) => ({ inc: (f * r) / SR, ph: 0, a: amps[k] * vel, d: Math.exp(-1 / (taus[k] * SR)) }))
    .filter((q) => q.inc * SR < 16000);
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    let v = 0;
    for (const q of P) {
      v += q.a * look(SINE, q.ph);
      q.ph += q.inc;
      q.a *= q.d;
    }
    v *= Math.min(1, i / (0.002 * SR));
    BELL[0][s] += v * gl;
    BELL[1][s] += v * gr;
  }
}

function kick(t0, vel = 1) {
  const s0 = S(t0);
  const len = Math.min(N - s0, S(0.55));
  let ph = 0;
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    const t = i / SR;
    const f = 46 + 95 * Math.exp(-t / 0.032);
    ph += f / SR;
    const env = Math.exp(-t / 0.17) * Math.min(1, t / 0.0015);
    const click = noise() * Math.exp(-t / 0.0022) * 0.1;
    const v = (look(SINE, ph) * env + click) * vel;
    DRUM[0][s] += v;
    DRUM[1][s] += v;
  }
  const dl = Math.min(N - s0, S(0.42));
  for (let i = 0; i < dl; i++) {
    const t = i / SR;
    const g = 1 - 0.42 * Math.min(1, t / 0.008) * Math.exp(-t / 0.12);
    if (g < DUCK[s0 + i]) DUCK[s0 + i] = g;
  }
}

function hat(t0, vel, p = 0.22, decay = 0.028) {
  const [gl, gr] = pan(p);
  const s0 = S(t0);
  const len = Math.min(N - s0, S(decay * 7));
  let x1 = 0, y1 = 0, x2 = 0, y2 = 0;
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    const t = i / SR;
    const n = noise();
    const a = 0.6 * (y1 + n - x1); // two one-pole high-passes, ~5 kHz
    x1 = n;
    y1 = a;
    const b = 0.6 * (y2 + a - x2);
    x2 = a;
    y2 = b;
    const v = b * Math.exp(-t / decay) * vel * Math.min(1, t / 0.0006);
    DRUM[0][s] += v * gl;
    DRUM[1][s] += v * gr;
  }
}

/** RBJ biquad band-pass (0 dB peak). */
function bandpass(fc, q) {
  const w = (2 * Math.PI * fc) / SR;
  const al = Math.sin(w) / (2 * q);
  const a0 = 1 + al;
  return { b0: al / a0, b1: 0, b2: -al / a0, a1: (-2 * Math.cos(w)) / a0, a2: (1 - al) / a0, x1: 0, x2: 0, y1: 0, y2: 0 };
}
function biq(f, x) {
  const y = f.b0 * x + f.b1 * f.x1 + f.b2 * f.x2 - f.a1 * f.y1 - f.a2 * f.y2;
  f.x2 = f.x1;
  f.x1 = x;
  f.y2 = f.y1;
  f.y1 = y;
  return y;
}

function clap(t0, vel, p = -0.12) {
  const [gl, gr] = pan(p);
  const bp = bandpass(1500, 0.9);
  const s0 = S(t0);
  const len = Math.min(N - s0, S(0.5));
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    const t = i / SR;
    let env = 0;
    for (const o of [0, 0.011, 0.022]) if (t >= o) env += Math.exp(-(t - o) / 0.0045);
    if (t >= 0.028) env += 0.6 * Math.exp(-(t - 0.028) / 0.1);
    const v = biq(bp, noise()) * env * vel;
    DRUM[0][s] += v * gl;
    DRUM[1][s] += v * gr;
  }
}

/** Air moving: band-passed noise sweeping up into `t1`, a faint rising tone
 *  riding along (the light itself), panned along the flight. */
function whoosh(t0, t1, p0, p1, vel = 1, { lo = 280, hi = 4600, tail = 0.35, tone = 0.06, q = 1.4, curve = 2.4 } = {}) {
  const s0 = Math.max(0, S(t0));
  const s1 = Math.min(N, S(t1 + tail));
  let ic1 = 0, ic2 = 0, ph = 0;
  const k = 1 / q;
  for (let s = s0; s < s1; s++) {
    const t = s / SR;
    const u = clamp((t - t0) / (t1 - t0), 0, 1);
    const env = t <= t1 ? u ** curve : Math.exp(-(t - t1) / (tail / 4));
    const fc = lo * (hi / lo) ** (t <= t1 ? u : 1 - 0.45 * clamp((t - t1) / tail, 0, 1));
    const g = Math.tan((Math.PI * Math.min(fc, SR * 0.45)) / SR);
    const a1 = 1 / (1 + g * (g + k));
    const a2 = g * a1;
    const a3 = g * a2;
    const v3 = noise() - ic2;
    const v1 = a1 * ic1 + a2 * v3;
    const v2 = ic2 + a2 * ic1 + a3 * v3;
    ic1 = 2 * v1 - ic1;
    ic2 = 2 * v2 - ic2;
    ph += (170 * (5.2 ** u)) / SR;
    const v = (v1 + tone * look(SINE, ph)) * env * vel;
    const [gl, gr] = pan(p0 + (p1 - p0) * smooth(u));
    SFX[0][s] += v * gl;
    SFX[1][s] += v * gr;
  }
}

/** A soft low "bloom" under an arrival: a falling sub and a puff of noise. */
function impact(t0, vel = 1) {
  const s0 = S(t0);
  const len = Math.min(N - s0, S(2.0));
  let ph = 0, lp = 0;
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    const t = i / SR;
    ph += (38 + 26 * Math.exp(-t / 0.12)) / SR;
    lp += 0.04 * (noise() - lp);
    const v = (look(SINE, ph) * Math.exp(-t / 0.55) + lp * 2.2 * Math.exp(-t / 0.18)) * 0.4 * vel * Math.min(1, t / 0.004);
    SFX[0][s] += v;
    SFX[1][s] += v;
  }
}

/** A UI click: a short tick with a little body. */
function click(t0, vel = 1, p = 0, freq = 2400) {
  const [gl, gr] = pan(p);
  const s0 = S(t0);
  const len = Math.min(N - s0, S(0.05));
  let ph = 0;
  for (let i = 0; i < len; i++) {
    const s = s0 + i;
    const t = i / SR;
    ph += freq / SR;
    const v = (noise() * 0.5 * Math.exp(-t / 0.0012) + look(SINE, ph) * Math.exp(-t / 0.007) + 0.6 * look(SINE, ph * 0.25) * Math.exp(-t / 0.012)) * vel;
    SFX[0][s] += v * gl;
    SFX[1][s] += v * gr;
  }
}

/** A digit roll: one tick each time the first digit changes, slowing down
 *  exactly like RollDigits (v = 10·spins·(1 − E.out(p))). */
function rollTicks(at, dur, spins, vel, p) {
  let prev = null;
  for (let t = at; t <= at + dur; t += 0.0005) {
    const d = Math.floor(10 * spins * (1 - easeOut((t - at) / dur)));
    if (prev !== null && d !== prev) click(t, vel * (0.75 + 0.25 * rnd()), p, 3200 + 600 * rnd());
    prev = d;
  }
}

/** The light arriving: a soft thump and a bell chord from the harmony. */
function bloom(t, vel = 1) {
  const c = chordAt(t + 0.01);
  impact(t, 0.22 * vel);
  bell(t, c.arp[4] + 12, 0.1 * vel, -0.3);
  bell(t + 0.025, c.arp[6] + 12, 0.08 * vel, 0.3);
}

/* ── the score ────────────────────────────────────────────────────────── */

// Intro: the light streaks in and lands on the mark; Dmaj9 swells under the logo.
whoosh(0.0, 1.15, 0.85, -0.25, 0.5, { lo: 380, hi: 5600, tail: 0.5, tone: 0.05 });
impact(1.15, 0.85);
bell(1.16, 74, 0.16, -0.1);
bell(1.19, 81, 0.12, 0.15);
padChord(1.0, O + 0.1, CH.D.pad, 1.15, 1.9, 1.3);
bassNote(1.2, O - 1.45, 38, 0.07, 1.4, 0.5);
// "LanBeam" spells itself out, left to right
[74, 78, 81, 85, 86, 88, 90].forEach((m, i) => bell(2.32 + i * 0.045, m, 0.07 - i * 0.005, -0.5 + i * 0.16));
// the light leaves for the radar hub
whoosh(4.12, O, xpan(629), xpan(1100), 0.42, { tail: 0.3 });
impact(O, 0.45);

// Pad: one chord per bar until the window folds away.
for (let k = 0; bar(k) < STOP; k++) {
  const t0 = bar(k);
  const t1 = Math.min(bar(k + 1), STOP + 0.25);
  const breakdown = k >= 16 && k <= 17;
  padChord(t0, t1, PROG[k % 4].pad, breakdown ? 1.25 : 1, k === 0 ? 0.2 : 0.35, 1.0);
}

// Bass: held notes, eighths from the drop (beat 9), held again in the breakdown.
for (let n = 0; n < 85; n++) {
  const t = beat(n);
  const c = chordAt(t);
  const held = n < 9 || (n >= 64 && n < 76);
  const paused = n >= 24 && n < 26;
  if (held) {
    if (n % 4 === 0) bassNote(t, 1.85, c.bass, n < 9 ? 0.13 : 0.12, 0.04, 0.3);
  } else if (paused) {
    if (n === 24) bassNote(t, 0.9, c.bass, 0.09, 0.08, 0.25);
  } else {
    bassNote(t, 0.17, c.bass, 0.15);
    bassNote(t + 0.25, 0.17, n % 4 === 3 ? c.bass + 12 : c.bass, 0.11);
  }
}
bassNote(FINAL, 3.3, 38, 0.17, 0.01, 0.9);

// Arpeggio: eighths at first, sixteenths from the drop; silent while paused.
const PATTERN = [0, 2, 4, 3, 5, 4, 6, 5, 7, 5, 6, 4, 5, 3, 4, 2];
const ACCENT = [1, 0.55, 0.7, 0.55, 0.85, 0.55, 0.7, 0.55, 0.95, 0.55, 0.7, 0.55, 0.85, 0.55, 0.7, 0.6];
for (let st = 0; st < 85 * 4; st++) {
  const n = Math.floor(st / 4);
  const t = O + st * STEP + (rnd() - 0.5) * 0.006;
  const c = chordAt(t + 0.01);
  const i = st % 16;
  let vel = 0.105 * ACCENT[i];
  if (n < 9 || (n >= 64 && n < 72)) {
    if (st % 2) continue; // eighths
    vel *= n < 9 ? 0.85 : 0.7;
  } else if (n >= 24 && n < 26) {
    continue; // paused
  } else if (n >= 72 && n < 76) {
    vel *= 0.45 + 0.55 * ((st - 72 * 4) / 16); // build into the lift
  }
  const p = (i % 2 ? 0.28 : -0.28) * (0.6 + 0.4 * rnd());
  pluck(t, c.arp[PATTERN[i]], vel * (0.9 + 0.2 * rnd()), p, ARP, { bright: 1.35 });
  if (n >= 76) pluck(t, c.arp[PATTERN[(i + 3) % 16]] + 12, vel * 0.32, -p, ARP, { decay: 0.3 });
}

// Drums: in on the drop, out while paused, out for the breakdown, back for the lift.
for (let n = 9; n < 85; n++) {
  const t = beat(n);
  const paused = n >= 24 && n < 26;
  const breakdown = n >= 64 && n < 76;
  if (!paused && !breakdown) kick(t, n % 4 === 0 ? 0.46 : 0.4);
  if (((n >= 33 && n < 64) || (n >= 77 && n < 85)) && n % 2 === 1) clap(t, 0.12);
  for (let q = 0; q < 4; q++) {
    const h = t + q * STEP;
    if (paused || n < 19) continue;
    if (breakdown) {
      if (q === 2) hat(h, 0.065);
      continue;
    }
    hat(h, [0.03, 0.04, 0.068, 0.04][q] * (0.85 + 0.3 * rnd()), 0.22, q === 2 ? 0.045 : 0.026);
  }
}

// 01 — devices found as the sweep passes them (Discover.tsx foundAt, on 16ths)
[
  [5.03, 83, 1164],
  [5.655, 86, 1396],
  [6.03, 88, 1366],
  [6.53, 90, 1078],
  [7.405, 93, 814],
].forEach(([t, m, x]) => bell(t, m, 0.075, xpan(x)));
// the file lands on the Mac mini: the drums come in
click(9.28, 0.06, xpan(1078), 900);

// 02 — the codes roll in, are checked against each other, and match
rollTicks(11.45, 0.95, 2, 0.022, 0);
whoosh(12.73, 13.28, xpan(652), xpan(1455), 0.28, { lo: 700, hi: 6000, tail: 0.25, tone: 0.08 });
bell(13.28, 86, 0.13, 0.1);
bell(13.4, 93, 0.1, 0.2);
click(13.78, 0.05, xpan(1573)); // 接收

// 03 — transfer: pause, resume, done
click(16.78, 0.05, xpan(1391));
click(17.78, 0.05, xpan(1391));
[86, 90, 93].forEach((m, i) => bell(18.78 + i * 0.07, m, 0.11 - i * 0.015, -0.1 + i * 0.1));

// 04 — quick text: sent, flies over, lands in the other clipboard
click(23.28, 0.05, xpan(949));
whoosh(23.36, 23.78, xpan(660), xpan(1370), 0.3, { lo: 600, hi: 5200, tail: 0.25, tone: 0.08 });
bell(23.78, 88, 0.11, xpan(1490));
bell(23.86, 93, 0.08, xpan(1490));

// 05 — browser receive: scanned, the page opens, tapped, downloaded
whoosh(27.78, 28.28, xpan(1526), xpan(470), 0.3, { lo: 600, hi: 5200, tail: 0.25, tone: 0.08 });
bell(28.28, 86, 0.06, xpan(470));
click(29.28, 0.04, xpan(470), 1500);
bell(29.78, 83, 0.11, xpan(1250));
bell(29.88, 86, 0.09, xpan(1250));

// 06 — pairing: the code rolls, the card flips, a link is pasted, join, the code to compare
rollTicks(32.33, 1.0, 2, 0.02, xpan(1000));
whoosh(33.78, 34.3, -0.35, -0.05, 0.16, { lo: 900, hi: 3200, tail: 0.2, tone: 0, curve: 1.2 });
click(34.78, 0.06, xpan(200), 1100); // ⌘V
click(35.28, 0.05, xpan(1026)); // 加入
rollTicks(35.63, 0.9, 2, 0.02, xpan(720));

// 07 — trust circle: picked up, dropped in, trusted
click(38.78, 0.045, xpan(706), 1800);
click(39.78, 0.05, xpan(1114), 1400);
[85, 88, 93].forEach((m, i) => bell(39.8 + i * 0.07, m, 0.1 - i * 0.012, xpan(1114) + i * 0.1));

// 08 — the window: two wipes sweep across it
whoosh(43.28, 44.13, -0.35, 0.75, 0.24, { lo: 500, hi: 7000, tail: 0.3, tone: 0.03, curve: 1.4 });
whoosh(45.28, 46.13, -0.35, 0.75, 0.24, { lo: 500, hi: 7000, tail: 0.3, tone: 0.03, curve: 1.4 });

// Handoffs: each panel folds into the light, the light flies, the next one opens.
[
  [19.78, 960, 660],
  [24.78, 660, 1250],
  [30.78, 1250, 720],
  [36.78, 720, 1230],
  [41.28, 1230, 1215],
  [STOP, 1215, 631],
].forEach(([fold, x0, x1]) => {
  whoosh(fold + 0.05, fold + 1.0, xpan(x0), xpan(x1), 0.5, { tail: 0.4 });
  if (fold !== STOP) bloom(fold + 1.0);
});
impact(42.28, 0.3); // the lift

// Outro: the light lands on the mark; Dmaj9 rings out to the end.
impact(FINAL, 0.95);
padChord(FINAL, DUR - 0.6, CH.D.pad.concat([74]), 1.3, 0.05, 2.0);
[62, 69, 74, 78, 81, 85].forEach((m, i) => bell(FINAL + 0.02 + i * 0.06, m + 12, 0.1 - i * 0.008, -0.4 + i * 0.16));
[74, 78, 81].forEach((m, i) => pluck(FINAL + 0.5 + i * 0.25, m, 0.05, (i - 1) * 0.3, ARP, { decay: 0.8 }));

/* ── mix ──────────────────────────────────────────────────────────────── */

/** RBJ low-pass, re-tuned every 32 samples so the cutoff can move. */
function lowpass(buf, cutoffAt, q = 0.7) {
  for (const ch of buf) {
    let x1 = 0, x2 = 0, y1 = 0, y2 = 0, b0 = 0, b1 = 0, b2 = 0, a1 = 0, a2 = 0;
    for (let s = 0; s < N; s++) {
      if (s % 32 === 0) {
        const w = (2 * Math.PI * Math.min(cutoffAt(s / SR), SR * 0.45)) / SR;
        const al = Math.sin(w) / (2 * q);
        const c = Math.cos(w);
        const a0 = 1 + al;
        b0 = (1 - c) / 2 / a0;
        b1 = (1 - c) / a0;
        b2 = b0;
        a1 = (-2 * c) / a0;
        a2 = (1 - al) / a0;
      }
      const x = ch[s];
      const y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
      x2 = x1;
      x1 = x;
      y2 = y1;
      y1 = y;
      ch[s] = y;
    }
  }
}

const seg = (t, pts) => {
  // piecewise-smooth automation through [time, value] points
  if (t <= pts[0][0]) return pts[0][1];
  for (let i = 1; i < pts.length; i++) {
    const [ta, va] = pts[i - 1];
    const [tb, vb] = pts[i];
    if (t <= tb) return va + (vb - va) * smooth((t - ta) / (tb - ta));
  }
  return pts[pts.length - 1][1];
};

// The pad opens up with the logo, ducks while the transfer is paused, swells
// through the breakdown into the lift, and closes as the ending rings out.
lowpass(PAD, (t) =>
  seg(t, [
    [0, 450], [1.15, 600], [4.4, 2800], [16.6, 3200], [16.9, 900], [17.6, 900], [17.9, 3200],
    [36.8, 3000], [37.5, 1900], [41.0, 2600], [42.3, 4800], [47.2, 3800], [48.3, 3400], [53.4, 1100],
  ]) * (1 + 0.08 * Math.sin(t * 0.7)),
);
lowpass(BASS, () => 950);
lowpass(ARP, (t) => (t >= 36.78 && t < 42.28 ? seg(t, [[36.78, 3600], [42.28, 9000]]) : 8500));

for (const ch of [0, 1]) {
  for (let s = 0; s < N; s++) {
    PAD[ch][s] *= DUCK[s];
    BASS[ch][s] *= DUCK[s];
    ARP[ch][s] *= 1 - (1 - DUCK[s]) * 0.4;
  }
}

/** Ping-pong delay, dotted eighth, darkening on every repeat. */
function pingpong(inL, inR, time, fb, tone) {
  const D = Math.round(time * SR);
  const bl = new Float32Array(D), br = new Float32Array(D);
  const L = new Float32Array(N), R = new Float32Array(N);
  let i = 0, lpL = 0, lpR = 0;
  for (let s = 0; s < N; s++) {
    const dl = bl[i], dr = br[i];
    lpL += tone * (dl - lpL);
    lpR += tone * (dr - lpR);
    bl[i] = (inL[s] + inR[s]) * 0.5 + lpR * fb;
    br[i] = lpL * fb;
    L[s] = dl;
    R[s] = dr;
    if (++i >= D) i = 0;
  }
  return [L, R];
}

/** Freeverb (Jezar's tunings, rescaled to 48 kHz), with a short pre-delay. */
function freeverb(inL, inR, { room = 0.86, damp = 0.38, predelay = 0.02 } = {}) {
  const sc = SR / 44100;
  const combs = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
  const aps = [556, 441, 341, 225];
  const spread = 23;
  const fb = room * 0.28 + 0.7;
  const d1 = damp * 0.4;
  const d2 = 1 - d1;
  const mk = (len) => ({ b: new Float32Array(Math.round(len * sc)), i: 0, st: 0 });
  const cl = combs.map((c) => mk(c)), cr = combs.map((c) => mk(c + spread));
  const al = aps.map((a) => mk(a)), ar = aps.map((a) => mk(a + spread));
  const pd = Math.round(predelay * SR);
  const L = new Float32Array(N), R = new Float32Array(N);
  const comb = (c, x) => {
    const y = c.b[c.i];
    c.st = y * d2 + c.st * d1;
    c.b[c.i] = x + c.st * fb;
    if (++c.i >= c.b.length) c.i = 0;
    return y;
  };
  const allpass = (a, x) => {
    const bo = a.b[a.i];
    a.b[a.i] = x + bo * 0.5;
    if (++a.i >= a.b.length) a.i = 0;
    return bo - x;
  };
  for (let s = 0; s < N; s++) {
    const j = s - pd;
    const x = j >= 0 ? (inL[j] + inR[j]) * 0.015 : 0;
    let l = 0, r = 0;
    for (let c = 0; c < 8; c++) {
      l += comb(cl[c], x);
      r += comb(cr[c], x);
    }
    for (let a = 0; a < 4; a++) {
      l = allpass(al[a], l);
      r = allpass(ar[a], r);
    }
    L[s] = l;
    R[s] = r;
  }
  return [L, R];
}

const send = (parts) => {
  const L = new Float32Array(N), R = new Float32Array(N);
  for (const [b, g] of parts)
    for (let s = 0; s < N; s++) {
      L[s] += b[0][s] * g;
      R[s] += b[1][s] * g;
    }
  return [L, R];
};
const DLY = pingpong(...send([[ARP, 0.24], [BELL, 0.35]]), 0.375, 0.36, 0.35);
const REV = freeverb(...send([[PAD, 0.35], [ARP, 0.3], [BELL, 0.6], [DRUM, 0.07], [SFX, 0.32], [DLY, 0.4]]));

const mix = send([
  [PAD, 1],
  [BASS, 1],
  [ARP, 1],
  [BELL, 1],
  [DRUM, 1],
  [SFX, 1],
  [REV, 0.75],
  [DLY, 0.45],
]);

/* ── master: high-pass, gain, look-ahead limiter at -1 dBFS, fades ───── */

for (const ch of mix) {
  // RBJ high-pass, 30 Hz: nothing a speaker could play lives below it
  const w = (2 * Math.PI * 30) / SR;
  const al = Math.sin(w) / (2 * 0.707);
  const c = Math.cos(w);
  const a0 = 1 + al;
  const b0 = (1 + c) / 2 / a0, b1 = -(1 + c) / a0, b2 = b0, a1 = (-2 * c) / a0, a2 = (1 - al) / a0;
  let x1 = 0, x2 = 0, y1 = 0, y2 = 0;
  for (let s = 0; s < N; s++) {
    const x = ch[s];
    const y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
    x2 = x1;
    x1 = x;
    y2 = y1;
    y1 = y;
    ch[s] = y;
  }
}

const GAIN = 0.93; // lands the mix at about -15 LUFS integrated
if (process.env.SCORE_STATS) {
  const db = (x) => (20 * Math.log10(x + 1e-12)).toFixed(1);
  const stat = (b) => {
    let pk = 0, ss = 0;
    for (let s = 0; s < N; s++) {
      const v = Math.max(Math.abs(b[0][s]), Math.abs(b[1][s]));
      pk = Math.max(pk, v);
      ss += b[0][s] ** 2 + b[1][s] ** 2;
    }
    return `peak ${db(pk)} rms ${db(Math.sqrt(ss / (2 * N)))}`;
  };
  for (const [k, b] of Object.entries({ PAD, BASS, ARP, BELL, DRUM, SFX, REV, DLY, mix })) console.log(k.padEnd(5), stat(b));
}
const CEIL = 10 ** (-1 / 20);
const ahead = S(0.005);
const atkK = 1 - Math.exp(-1 / (0.0015 * SR));
const relK = Math.exp(-1 / (0.12 * SR));
const need = new Float32Array(N);
for (let s = 0; s < N; s++) {
  const pk = Math.max(Math.abs(mix[0][s]), Math.abs(mix[1][s])) * GAIN;
  need[s] = pk > CEIL ? CEIL / pk : 1;
}
// the gain at each sample must already be down for any peak in the next 5 ms
const gain = new Float32Array(N);
{
  const dq = []; // monotonic deque of indices, minimum of need[] over [s, s + ahead]
  let g = 1;
  for (let s = 0; s < N; s++) {
    const e = s + ahead;
    if (e < N) {
      while (dq.length && need[dq[dq.length - 1]] >= need[e]) dq.pop();
      dq.push(e);
    }
    while (dq.length && dq[0] < s) dq.shift();
    const target = Math.min(1, dq.length ? need[dq[0]] : 1, need[s]);
    // ease down over the look-ahead (no step), recover slowly
    g = target < g ? g + (target - g) * atkK : target + (g - target) * relK;
    gain[s] = g;
  }
}
const fadeIn = S(0.01);
const fadeOutAt = S(51.9);
for (let s = 0; s < N; s++) {
  let f = 1;
  if (s < fadeIn) f = s / fadeIn;
  if (s > fadeOutAt) f = 1 - smooth((s - fadeOutAt) / (N - fadeOutAt));
  const g = GAIN * gain[s] * f;
  // whatever the easing let through is caught here
  mix[0][s] = clamp(mix[0][s] * g, -CEIL, CEIL);
  mix[1][s] = clamp(mix[1][s] * g, -CEIL, CEIL);
}

/* ── write ────────────────────────────────────────────────────────────── */

const buf = Buffer.alloc(44 + N * 4);
buf.write("RIFF", 0);
buf.writeUInt32LE(36 + N * 4, 4);
buf.write("WAVE", 8);
buf.write("fmt ", 12);
buf.writeUInt32LE(16, 16);
buf.writeUInt16LE(1, 20);
buf.writeUInt16LE(2, 22);
buf.writeUInt32LE(SR, 24);
buf.writeUInt32LE(SR * 4, 28);
buf.writeUInt16LE(4, 32);
buf.writeUInt16LE(16, 34);
buf.write("data", 36);
buf.writeUInt32LE(N * 4, 40);
let peak = 0;
for (let s = 0; s < N; s++) {
  for (const ch of [0, 1]) {
    const x = mix[ch][s];
    peak = Math.max(peak, Math.abs(x));
    const v = Math.round(x * 32767 + (rnd() - rnd()) * 0.5); // TPDF dither
    buf.writeInt16LE(clamp(v, -32768, 32767), 44 + s * 4 + ch * 2);
  }
}
if (process.env.SCORE_STATS) {
  let red = 0, worst = 1;
  for (let s = 0; s < N; s++) {
    if (gain[s] < 0.891) red++;
    worst = Math.min(worst, gain[s]);
  }
  console.log(`limiter: >1 dB on ${((100 * red) / N).toFixed(1)}% of samples, deepest ${(20 * Math.log10(worst)).toFixed(1)} dB`);
}
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, buf);
console.log(`score: ${OUT} · ${DUR}s · peak ${(20 * Math.log10(peak)).toFixed(1)} dBFS`);
