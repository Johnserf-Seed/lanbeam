import { useEffect, useState } from "react";
import { AbsoluteFill, continueRender, delayRender, Html5Audio, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { Background } from "./components/Background";
import { HandoffComet } from "./components/Comet";
import { E, lerp, prog } from "./lib/anim";
import { H, inSpan, LOOK_C, PAIR_C, RADAR_C, SEND_C, SHARE_C, T, TEXT_C, TRUST_C } from "./lib/timeline";
import { Discover } from "./scenes/Discover";
import { Intro } from "./scenes/Intro";
import { Look } from "./scenes/Look";
import { Outro } from "./scenes/Outro";
import { Pair } from "./scenes/Pair";
import { QuickText } from "./scenes/QuickText";
import { Send } from "./scenes/Send";
import { Share } from "./scenes/Share";
import { Trust } from "./scenes/Trust";

/** Hold the first frame until the Latin faces are in (CJK comes from the
 *  system, like in the app). */
function useFonts() {
  const [handle] = useState(() => delayRender("fonts"));
  useEffect(() => {
    const faces = [
      '400 24px "Space Grotesk Variable"',
      '600 24px "Space Grotesk Variable"',
      '400 24px "IBM Plex Mono"',
      '500 24px "IBM Plex Mono"',
      '600 24px "IBM Plex Mono"',
    ];
    Promise.all(faces.map((f) => document.fonts.load(f, "LanBeam 0123456789 MB/s")))
      .then(() => document.fonts.ready)
      .then(() => continueRender(handle))
      .catch(() => continueRender(handle));
  }, [handle]);
}

// The pool of light behind the action drifts from scene to scene.
const SPOTS: [number, { x: number; y: number }][] = [
  [0, { x: 960, y: 520 }],
  [4.2, { x: 960, y: 520 }],
  [4.9, RADAR_C],
  [10.0, RADAR_C],
  [10.8, { x: 1010, y: 600 }],
  [14.4, { x: 1010, y: 600 }],
  [15.4, SEND_C],
  [H.sendText.t0, SEND_C],
  [H.sendText.t1, TEXT_C],
  [H.textShare.t0, TEXT_C],
  [H.textShare.t1, SHARE_C],
  [H.sharePair.t0, SHARE_C],
  [H.sharePair.t1, PAIR_C],
  [H.pairTrust.t0, PAIR_C],
  [H.pairTrust.t1, TRUST_C],
  [H.trustLook.t0, TRUST_C],
  [H.trustLook.t1, LOOK_C],
  [H.lookOutro.t0, LOOK_C],
  [H.lookOutro.t1, { x: 960, y: 520 }],
];

function spotAt(t: number) {
  for (let i = 1; i < SPOTS.length; i++) {
    const [t1, b] = SPOTS[i];
    const [t0, a] = SPOTS[i - 1];
    if (t <= t1) {
      const p = prog(t, t0, t1, E.inOut);
      return { x: lerp(a.x, b.x, p), y: lerp(a.y, b.y, p) };
    }
  }
  return SPOTS[SPOTS.length - 1][1];
}

export function Promo() {
  useFonts();
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = frame / fps;

  const local = (s: readonly [number, number]) => t - s[0];
  const discoverExit = prog(t, T.discover[1] - 0.5, T.discover[1], E.inOut);
  const power = prog(t, 0.6, 2.5) * (1 - 0.7 * prog(t, T.outro[1] - 1.0, T.outro[1]));

  return (
    <AbsoluteFill style={{ background: "#04070a" }}>
      {/* synthesized by scripts/score.mjs, cut to the cues in lib/timeline.ts */}
      <Html5Audio src={staticFile("audio/score.wav")} />
      <Background spot={spotAt(t)} spotPower={power} gridOpacity={power} />
      {inSpan(t, T.intro) && <Intro t={local(T.intro)} />}
      {inSpan(t, T.discover) && <Discover t={local(T.discover)} exit={discoverExit} />}
      {inSpan(t, T.send) && <Send t={local(T.send)} />}
      {inSpan(t, T.text) && <QuickText t={local(T.text)} />}
      {inSpan(t, T.share) && <Share t={local(T.share)} />}
      {inSpan(t, T.pair) && <Pair t={local(T.pair)} />}
      {inSpan(t, T.trust) && <Trust t={local(T.trust)} />}
      {inSpan(t, T.look) && <Look t={local(T.look)} />}
      {inSpan(t, T.outro) && <Outro t={local(T.outro)} />}
      {Object.values(H).map((h) => (
        <HandoffComet key={h.fold} t={t} h={h} />
      ))}
    </AbsoluteFill>
  );
}
