import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
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
