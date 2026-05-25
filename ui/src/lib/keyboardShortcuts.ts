// App-level keyboard shortcuts.
//
// We attach a single window-level keydown listener on mount and tear
// it down on dismount. Inputs / textareas / contenteditable elements
// are skipped so the user can still type freely; only "neutral"
// keypresses (no input focused) trigger transport / search shortcuts.

import { app } from "./stores.svelte";
import { nowPlayingFullscreen } from "./nowPlayingFullscreen.svelte";
import { nextTrack, pause, prevTrack, resume, seek } from "./api";

function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return target.isContentEditable;
}

/**
 * Install the global shortcut listener. Returns a cleanup function
 * the caller is expected to invoke on unmount.
 */
export function installKeyboardShortcuts(): () => void {
  function onKeyDown(e: KeyboardEvent): void {
    // Ctrl/Cmd-F: focus the search bar — works even when typing in
    // a text field elsewhere.
    if ((e.ctrlKey || e.metaKey) && (e.key === "f" || e.key === "F")) {
      e.preventDefault();
      app.setView("search");
      // The SearchBar lives in the always-mounted TitleBar; focus
      // its input on the next frame so the view transition has
      // settled and any other focused element has been blurred.
      requestAnimationFrame(() => {
        const input = document.querySelector<HTMLInputElement>(
          ".search input[type=\"search\"]"
        );
        input?.focus();
        input?.select();
      });
      return;
    }

    if (isTypingTarget(e.target)) return;

    switch (e.key) {
      case " ": {
        // Space: toggle play/pause. Only when not typing, otherwise
        // we'd swallow legitimate spacebar input.
        e.preventDefault();
        const status = app.player.status;
        if (status === "playing") void pause();
        else if (status === "paused") void resume();
        break;
      }
      case "ArrowRight": {
        // ALT+→ / Ctrl+→ : next track. Plain → also seeks +5s.
        if (e.altKey || e.ctrlKey) {
          e.preventDefault();
          void nextTrack();
        } else {
          e.preventDefault();
          const dur = app.player.duration_seconds;
          if (dur > 0) {
            const target = Math.min(dur, app.player.position_seconds + 5);
            void seek(target);
          }
        }
        break;
      }
      case "ArrowLeft": {
        if (e.altKey || e.ctrlKey) {
          e.preventDefault();
          void prevTrack();
        } else {
          e.preventDefault();
          const target = Math.max(0, app.player.position_seconds - 5);
          void seek(target);
        }
        break;
      }
      case "n":
      case "N": {
        // Plain "n" — next track without modifiers. Common in
        // music apps and easier than Alt+→.
        e.preventDefault();
        void nextTrack();
        break;
      }
      case "p":
      case "P": {
        e.preventDefault();
        void prevTrack();
        break;
      }
      case "f":
      case "F": {
        // Toggle the immersive Now Playing screen.
        e.preventDefault();
        nowPlayingFullscreen.toggle();
        break;
      }
      case "Escape": {
        if (nowPlayingFullscreen.open) {
          e.preventDefault();
          nowPlayingFullscreen.hide();
        } else if (app.searchQuery) {
          e.preventDefault();
          app.searchQuery = "";
        }
        break;
      }
      default:
        break;
    }
  }

  window.addEventListener("keydown", onKeyDown);
  return () => window.removeEventListener("keydown", onKeyDown);
}
