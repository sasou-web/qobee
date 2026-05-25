import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App.svelte";
import MiniPlayer from "./components/MiniPlayer.svelte";

const target = document.getElementById("app");
if (!target) {
  throw new Error("missing #app root element");
}

// One Vite entry, two Tauri windows. We pick which Svelte root to
// render based on the window label set in tauri.conf.json. This keeps
// the dev workflow simple (one bundle, one HMR connection) while
// letting the mini player live in a separate native window.
let label = "main";
try {
  label = getCurrentWindow().label;
} catch {
  // Running outside Tauri (e.g. plain `vite preview`): default to main.
}

if (label === "mini") {
  mount(MiniPlayer, { target });
} else {
  mount(App, { target });
}
