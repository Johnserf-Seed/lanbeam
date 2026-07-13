/** WebAudio notification chimes (from the design prototype). */
export type SoundKind = "叮咚" | "清脆叮" | "水滴" | "木鱼";

let ac: AudioContext | null = null;

export function playSound(kind: SoundKind): void {
  try {
    if (!ac) ac = new AudioContext();
    const ctx = ac;
    if (ctx.state === "suspended") void ctx.resume();
    const t0 = ctx.currentTime;
    const tone = (
      f0: number,
      at: number,
      dur: number,
      type: OscillatorType = "sine",
      f1?: number,
      vol?: number,
    ) => {
      const o = ctx.createOscillator(),
        g = ctx.createGain();
      o.type = type;
      o.frequency.setValueAtTime(f0, t0 + at);
      if (f1) o.frequency.exponentialRampToValueAtTime(f1, t0 + at + dur);
      g.gain.setValueAtTime(0, t0 + at);
      g.gain.linearRampToValueAtTime(vol || 0.2, t0 + at + 0.012);
      g.gain.exponentialRampToValueAtTime(0.0008, t0 + at + dur);
      o.connect(g);
      g.connect(ctx.destination);
      o.start(t0 + at);
      o.stop(t0 + at + dur + 0.05);
    };
    if (kind === "清脆叮") tone(1318.5, 0, 0.5);
    else if (kind === "水滴") tone(640, 0, 0.2, "sine", 290, 0.26);
    else if (kind === "木鱼") tone(420, 0, 0.09, "triangle", 300, 0.32);
    else {
      tone(659.3, 0, 0.3);
      tone(880, 0.13, 0.5);
    }
  } catch {
    /* audio unavailable */
  }
}
