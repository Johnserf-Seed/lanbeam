// The app's version, from package.json — the one `pnpm version:set` writes
// (along with tauri.conf.json and Cargo.toml, which version.test.ts holds to
// it). Vite inlines just this field. The About section used to hard-code
// "v0.1.0" and kept saying so through 0.1.3 (M14).
import { version } from "../../package.json";

export const APP_VERSION: string = version;
