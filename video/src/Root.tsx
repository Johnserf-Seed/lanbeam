import "@fontsource-variable/space-grotesk";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@fontsource/ibm-plex-mono/600.css";
import { Composition } from "remotion";
import { Promo } from "./Promo";
import { DURATION } from "./lib/timeline";

export const FPS = 60;

export function Root() {
  return (
    <Composition
      id="LanBeamPromo"
      component={Promo}
      durationInFrames={Math.round(DURATION * FPS)}
      fps={FPS}
      width={1920}
      height={1080}
    />
  );
}
