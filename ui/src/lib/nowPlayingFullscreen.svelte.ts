// Tiny store driving the immersive Now Playing screen overlay.
// Triggered by clicking the cover in the player bar (or pressing F).
// Kept here (not in stores.svelte) so the full-screen view can be
// toggled from anywhere without dragging the whole app store in.

class NowPlayingFullscreen {
  open = $state<boolean>(false);

  toggle(): void {
    this.open = !this.open;
  }
  show(): void {
    this.open = true;
  }
  hide(): void {
    this.open = false;
  }
}

export const nowPlayingFullscreen = new NowPlayingFullscreen();
