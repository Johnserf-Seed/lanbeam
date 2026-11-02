import { Config } from "@remotion/cli/config";

// H.264 at a high quality; JPEG frames would band on the dark gradients, so the
// frames go to the encoder as PNG.
Config.setVideoImageFormat("png");
Config.setCodec("h264");
Config.setCrf(16);
Config.setPixelFormat("yuv420p");
Config.setConcurrency(null);
