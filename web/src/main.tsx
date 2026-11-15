import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
// Space Grotesk ships with the app (OFL 1.1), for the Visions headings on every OS.
import "@fontsource/space-grotesk/latin-500.css";
import "@fontsource/space-grotesk/latin-700.css";
import "./theme/tokens.css";
import { applyAppearance, rememberedPalette, resolveSky } from "./theme/appearance";
import { DEFAULT_SETTINGS } from "./state/reducer";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root element");
}

// Wear the look chosen last time before the first paint, so the startup screen opens on the right sky.
applyAppearance({ ...DEFAULT_SETTINGS, theme_palette: resolveSky(rememberedPalette()) });

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
