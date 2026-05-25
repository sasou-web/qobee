// Global store for the "pick a playlist" modal triggered by the
// "Add to Playlist…" context menu entry.

interface PlaylistPickerState {
  open: boolean;
  trackIds: number[];
}

class PlaylistPickerStore {
  state = $state<PlaylistPickerState>({ open: false, trackIds: [] });

  show(trackIds: number[]): void {
    if (trackIds.length === 0) return;
    this.state = { open: true, trackIds: trackIds.slice() };
  }

  close(): void {
    this.state = { ...this.state, open: false };
  }
}

export const playlistPicker = new PlaylistPickerStore();
