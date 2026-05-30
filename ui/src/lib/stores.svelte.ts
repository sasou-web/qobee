// Reactive stores for the Qobee UI.
//
// Uses Svelte 5 runes via `$state` so consumers can read these as plain
// values; they're cheap to import anywhere.

import {
  getPlayerState,
  listAlbums,
  listArtists,
  listGenres,
  listLibraryRoots,
  listPlaylists,
  onPlayerEndOfTrack,
  onPlayerError,
  onPlayerPosition,
  onPlayerState,
  onScanError,
  onScanFinished,
  onScanProgress,
  onScanStarted,
  recentlyPlayedAlbums,
  recentlyPlayedArtists,
  recentlyPlayedTracks,
  type Album,
  type Artist,
  type Genre,
  type PlayerState,
  type Playlist,
  type ScanProgress,
  type ScanResult,
  type Track,
} from "./api";
import { toasts } from "./toasts.svelte";

export type ViewKey =
  | "home"
  | "library"
  | "artists"
  | "artist-detail"
  | "albums"
  | "album-detail"
  | "genres"
  | "genre-detail"
  | "playlists"
  | "playlist-detail"
  | "favorites"
  | "queue"
  | "search"
  | "settings";

/** Route snapshot stored in the navigation history. Captures every
 *  field that drives the current view so going back fully restores
 *  the previous page (e.g. the right artist, album or genre). */
export interface Route {
  view: ViewKey;
  albumId: number | null;
  genre: string | null;
  artist: string | null;
  playlistId: number | null;
}

const initialPlayer: PlayerState = {
  status: "idle",
  current_track_id: null,
  position_seconds: 0,
  duration_seconds: 0,
  volume: 1,
  output_mode: "shared",
  sample_rate: null,
  bit_depth: null,
  channels: null,
  is_bit_perfect: false,
  rg_attenuation_db: null,
  error: null,
};

class AppStore {
  player = $state<PlayerState>(initialPlayer);
  albums = $state<Album[]>([]);
  artists = $state<Artist[]>([]);
  scan = $state<ScanProgress | null>(null);
  scanRunning = $state<boolean>(false);
  lastError = $state<string | null>(null);
  selectedAlbumId = $state<number | null>(null);
  selectedView = $state<ViewKey>("home");

  // Home / discovery
  recentTracks = $state<Track[]>([]);
  recentAlbums = $state<Album[]>([]);
  recentArtists = $state<Artist[]>([]);
  genres = $state<Genre[]>([]);

  // Genre browsing
  selectedGenre = $state<string | null>(null);

  // Artist browsing
  selectedArtist = $state<string | null>(null);

  // Playlists
  playlists = $state<Playlist[]>([]);
  selectedPlaylistId = $state<number | null>(null);

  // Search
  searchQuery = $state<string>("");

  /** True when the user has at least one configured library root.
   *  Drives the Welcome / onboarding screen. */
  hasLibraryRoots = $state<boolean>(false);
  /** True once we've checked at least once whether the library has
   *  roots configured. Prevents a Welcome flash on first load. */
  rootsChecked = $state<boolean>(false);

  // Navigation history. Each entry is a snapshot of the previous
  // route, oldest first. We push to it whenever the user navigates
  // forward (clicking an album from a genre, an artist from a track…)
  // so the per-page "← Back" button can return to the actual page
  // they came from instead of always landing on a fixed parent.
  history = $state<Route[]>([]);

  /** True when we have at least one entry to return to. */
  canGoBack = $derived(this.history.length > 0);

  // Internal flag set by goBack() so navigation methods know to
  // *replace* the current route instead of pushing it back onto the
  // history. Without this, hitting Back would push the page we're
  // leaving back into history, leading to a forward/back ping-pong.
  private replacing = false;

  private unlisten: Array<() => void> = [];
  private wired = false;

  async wire(): Promise<void> {
    if (this.wired) return;
    this.wired = true;

    try {
      this.player = await getPlayerState();
    } catch (e) {
      this.lastError = String(e);
    }

    this.unlisten.push(
      await onPlayerState((s) => {
        this.player = s;
      })
    );
    this.unlisten.push(
      await onPlayerPosition((p) => {
        this.player = { ...this.player, position_seconds: p };
      })
    );
    this.unlisten.push(
      await onPlayerEndOfTrack(async () => {
        // Refresh "recently played" when a track ends so the home page
        // stays current without polling.
        await this.refreshHome();
      })
    );
    this.unlisten.push(
      await onPlayerError((msg) => {
        // R9.6 — the WASAPI Exclusive → Shared fallback surfaces as a
        // legacy `player:error` carrying the FR reason republished by
        // core (`EngineEvent::ExclusiveFallback`). It's an informational
        // soft fallback, not a hard failure, so route it to a transient
        // toast (announced via the ToastsRoot live region) instead of
        // the sticky red error banner. Other errors keep the banner.
        if (msg.startsWith("Mode Exclusive indisponible")) {
          toasts.warn(msg);
          return;
        }
        this.lastError = msg;
      })
    );
    this.unlisten.push(
      await onScanProgress((p) => {
        this.scan = p;
      })
    );
    this.unlisten.push(
      await onScanStarted(() => {
        // Concurrency guard (R6.4): any rescan trigger — not only the
        // Settings button — flips the "running" flag so a second
        // concurrent rescan can't be launched. The "started" toast is
        // raised at trigger time in Settings.handleRescanAll (R6.1) so
        // the user gets immediate feedback; we don't duplicate it here.
        this.scanRunning = true;
      })
    );
    this.unlisten.push(
      await onScanFinished(async (result: ScanResult) => {
        this.scanRunning = false;
        // R6.2: report how many tracks changed. `files_indexed` is the
        // closest "changed" count the scanner surfaces (added/updated);
        // a finer add/remove/update breakdown isn't available.
        const changed = result.files_indexed;
        toasts.success(`Scan terminé · ${changed} titres modifiés`);
        await this.refreshAll();
      })
    );
    this.unlisten.push(
      await onScanError((message) => {
        // R6.3: global rescan failure — release the guard and surface a
        // descriptive French error toast.
        this.scanRunning = false;
        toasts.error(`Échec du scan : ${message}`);
      })
    );
  }

  destroy(): void {
    for (const fn of this.unlisten) {
      try {
        fn();
      } catch {
        // ignore
      }
    }
    this.unlisten = [];
    this.wired = false;
  }

  async refreshLibrary(): Promise<void> {
    try {
      const [albums, artists] = await Promise.all([
        listAlbums(),
        listArtists(),
      ]);
      this.albums = albums;
      this.artists = artists;
    } catch (e) {
      this.lastError = String(e);
    }
  }

  async refreshHome(): Promise<void> {
    try {
      const [tracks, albums, artists, genres] = await Promise.all([
        recentlyPlayedTracks(20),
        recentlyPlayedAlbums(8),
        recentlyPlayedArtists(8),
        listGenres(),
      ]);
      this.recentTracks = tracks;
      this.recentAlbums = albums;
      this.recentArtists = artists;
      this.genres = genres;
    } catch (e) {
      this.lastError = String(e);
    }
  }

  async refreshPlaylists(): Promise<void> {
    try {
      this.playlists = await listPlaylists();
    } catch (e) {
      this.lastError = String(e);
    }
  }

  async refreshAll(): Promise<void> {
    await Promise.all([
      this.refreshLibrary(),
      this.refreshHome(),
      this.refreshPlaylists(),
      this.refreshLibraryRoots(),
    ]);
  }

  /** Pull the list of configured library roots from the backend.
   *  Used to decide whether to render the Welcome screen. */
  async refreshLibraryRoots(): Promise<void> {
    try {
      const roots = await listLibraryRoots();
      this.hasLibraryRoots = roots.length > 0;
    } catch {
      // best-effort
    } finally {
      this.rootsChecked = true;
    }
  }

  /** Snapshot the current route. Used to push entries onto the
   *  history stack before navigating somewhere new. */
  private currentRoute(): Route {
    return {
      view: this.selectedView,
      albumId: this.selectedAlbumId,
      genre: this.selectedGenre,
      artist: this.selectedArtist,
      playlistId: this.selectedPlaylistId,
    };
  }

  /** Push the current route onto the history stack, unless we're in
   *  the middle of a Back navigation (which already replaced the
   *  route). Trims the stack to a sane length to bound memory. */
  private remember(): void {
    if (this.replacing) return;
    const cur = this.currentRoute();
    // Don't record duplicates: navigating to the same place we're
    // already on (e.g. clicking the active sidebar item) shouldn't
    // grow the stack.
    const top = this.history[this.history.length - 1];
    if (top && routeEq(top, cur)) return;
    this.history = [...this.history.slice(-49), cur];
  }

  /** Apply a Route snapshot to the live state in one shot, without
   *  pushing the current route to history. */
  private applyRoute(r: Route): void {
    this.selectedAlbumId = r.albumId;
    this.selectedGenre = r.genre;
    this.selectedArtist = r.artist;
    this.selectedPlaylistId = r.playlistId;
    this.selectedView = r.view;
  }

  /** Pop the most recent route off the history and navigate to it.
   *  Used by every "← Back" button. Falls back to Home when the
   *  history is empty (e.g. deep-linked entry). */
  goBack(): void {
    const prev = this.history[this.history.length - 1];
    if (!prev) {
      // Nothing to return to — fall back to Home.
      this.replacing = true;
      this.applyRoute({
        view: "home",
        albumId: null,
        genre: null,
        artist: null,
        playlistId: null,
      });
      this.replacing = false;
      return;
    }
    this.history = this.history.slice(0, -1);
    this.replacing = true;
    this.applyRoute(prev);
    this.replacing = false;
  }

  selectAlbum(id: number | null): void {
    this.remember();
    this.selectedAlbumId = id;
    this.selectedView = id === null ? "albums" : "album-detail";
  }

  selectGenre(name: string | null): void {
    this.remember();
    this.selectedGenre = name;
    this.selectedView = name === null ? "genres" : "genre-detail";
  }

  selectArtist(name: string | null): void {
    this.remember();
    this.selectedArtist = name;
    this.selectedView = name === null ? "artists" : "artist-detail";
  }

  selectPlaylist(id: number | null): void {
    this.remember();
    this.selectedPlaylistId = id;
    this.selectedView = id === null ? "playlists" : "playlist-detail";
  }

  setView(view: ViewKey): void {
    this.remember();
    this.selectedView = view;
    if (view !== "album-detail") this.selectedAlbumId = null;
    if (view !== "genre-detail") this.selectedGenre = null;
    if (view !== "playlist-detail") this.selectedPlaylistId = null;
    if (view !== "artist-detail") this.selectedArtist = null;
  }

  beginScan(): void {
    this.scanRunning = true;
    this.scan = { files_visited: 0, files_indexed: 0, current: null };
  }
}

export const app = new AppStore();

function routeEq(a: Route, b: Route): boolean {
  return (
    a.view === b.view &&
    a.albumId === b.albumId &&
    a.genre === b.genre &&
    a.artist === b.artist &&
    a.playlistId === b.playlistId
  );
}
