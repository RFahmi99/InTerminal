import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/atkinson-hyperlegible-next";
import "@fontsource-variable/atkinson-hyperlegible-mono";
import "./styles/app.css";
import App from "./App";

// xterm.js measures character cells when it opens, so the terminal font must
// be loaded first or the grid is sized for the fallback font.
const MONO_FONT = '16px "Atkinson Hyperlegible Mono Variable"';

function render() {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

document.fonts
  .load(MONO_FONT)
  .catch((error) => {
    // The fallback monospace font still works; the grid just looks plainer.
    console.warn(`Terminal font failed to load, using a fallback: ${error}`);
  })
  .finally(render);
