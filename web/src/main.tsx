import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { DesktopCompanion } from "./components/DesktopCompanion";
import "./theme/tokens.css";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root element");
}

if (document.documentElement.dataset.theme !== "light") {
  document.documentElement.dataset.theme = "dark";
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    {new URLSearchParams(window.location.search).has("companion") ? <DesktopCompanion /> : <App />}
  </React.StrictMode>,
);
