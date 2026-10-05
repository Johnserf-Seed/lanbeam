import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
// Fonts are bundled, not fetched: a Google Fonts <link> made every launch of a
// "nothing leaves your LAN" app phone a third-party CDN. Space Grotesk is the
// variable build (one woff2 per subset covers every weight); Plex Mono only
// renders IDs, IPs, sizes and codes, so the Latin subset of the three weights
// the UI uses is all it needs.
import "@fontsource-variable/space-grotesk";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource/ibm-plex-mono/latin-600.css";
import "./styles.css";
import "./i18n";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
