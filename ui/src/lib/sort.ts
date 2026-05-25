// Sort helpers for the Albums and Genres tabs.
//
// Each sort mode is a tuple of (criterion, direction). The criterion
// drives which fields are compared and in what order; the direction
// flips the sign once at the end. Persistence is a tiny localStorage
// wrapper so users keep their choice across reloads without paying a
// roundtrip to the SQLite settings table for a UI-only preference.

import type { Album, Artist, Genre } from "./api";

export type SortDirection = "asc" | "desc";

// ---------------------------------------------------------------------------
// Albums
// ---------------------------------------------------------------------------

export type AlbumSortField =
  | "date_added"
  | "name"
  | "artist"
  | "release_date"
  | "genre"
  | "genre_release_date";

export interface AlbumSortOption {
  key: AlbumSortField;
  label: string;
}

export const ALBUM_SORT_OPTIONS: AlbumSortOption[] = [
  { key: "date_added", label: "Date Added" },
  { key: "name", label: "Name" },
  { key: "artist", label: "Artist" },
  { key: "release_date", label: "Release Date" },
  { key: "genre", label: "Genre" },
  { key: "genre_release_date", label: "Genre, Release Date" },
];

const collator = new Intl.Collator(undefined, {
  sensitivity: "base",
  numeric: true,
});

function cmpStr(a: string | null | undefined, b: string | null | undefined): number {
  // Empty / missing values always sink to the bottom in ascending order
  // so a "Genre" sort doesn't put untagged albums on top.
  const aEmpty = !a;
  const bEmpty = !b;
  if (aEmpty && bEmpty) return 0;
  if (aEmpty) return 1;
  if (bEmpty) return -1;
  return collator.compare(a as string, b as string);
}

function cmpNum(a: number | null | undefined, b: number | null | undefined): number {
  const aN = a == null ? Number.NEGATIVE_INFINITY : a;
  const bN = b == null ? Number.NEGATIVE_INFINITY : b;
  if (aN < bN) return -1;
  if (aN > bN) return 1;
  return 0;
}

function compareAlbums(a: Album, b: Album, field: AlbumSortField): number {
  switch (field) {
    case "date_added":
      return cmpNum(a.date_added, b.date_added);
    case "name":
      return cmpStr(a.title, b.title);
    case "artist":
      // Artist primary, then year, then album title to keep a stable
      // visual order when the artist has many records.
      return (
        cmpStr(a.artist, b.artist) ||
        cmpNum(a.year, b.year) ||
        cmpStr(a.title, b.title)
      );
    case "release_date":
      return cmpNum(a.year, b.year) || cmpStr(a.title, b.title);
    case "genre":
      return cmpStr(a.genre, b.genre) || cmpStr(a.title, b.title);
    case "genre_release_date":
      return (
        cmpStr(a.genre, b.genre) ||
        cmpNum(a.year, b.year) ||
        cmpStr(a.title, b.title)
      );
  }
}

export function sortAlbums(
  albums: readonly Album[],
  field: AlbumSortField,
  direction: SortDirection
): Album[] {
  const sign = direction === "asc" ? 1 : -1;
  const out = albums.slice();
  out.sort((a, b) => sign * compareAlbums(a, b, field));
  return out;
}

// ---------------------------------------------------------------------------
// Artists
// ---------------------------------------------------------------------------

export type ArtistSortField = "name" | "album_count" | "track_count";

export interface ArtistSortOption {
  key: ArtistSortField;
  label: string;
}

export const ARTIST_SORT_OPTIONS: ArtistSortOption[] = [
  { key: "name", label: "Name" },
  { key: "album_count", label: "Album Count" },
  { key: "track_count", label: "Track Count" },
];

function compareArtists(a: Artist, b: Artist, field: ArtistSortField): number {
  switch (field) {
    case "name":
      return cmpStr(a.name, b.name);
    case "album_count":
      return cmpNum(a.album_count, b.album_count) || cmpStr(a.name, b.name);
    case "track_count":
      return cmpNum(a.track_count, b.track_count) || cmpStr(a.name, b.name);
  }
}

export function sortArtists(
  artists: readonly Artist[],
  field: ArtistSortField,
  direction: SortDirection
): Artist[] {
  const sign = direction === "asc" ? 1 : -1;
  const out = artists.slice();
  out.sort((a, b) => sign * compareArtists(a, b, field));
  return out;
}

// ---------------------------------------------------------------------------
// Genres
// ---------------------------------------------------------------------------

export type GenreSortField = "name" | "track_count";

export interface GenreSortOption {
  key: GenreSortField;
  label: string;
}

export const GENRE_SORT_OPTIONS: GenreSortOption[] = [
  { key: "name", label: "Name" },
  { key: "track_count", label: "Track Count" },
];

function compareGenres(a: Genre, b: Genre, field: GenreSortField): number {
  switch (field) {
    case "name":
      return cmpStr(a.name, b.name);
    case "track_count":
      return cmpNum(a.track_count, b.track_count) || cmpStr(a.name, b.name);
  }
}

export function sortGenres(
  genres: readonly Genre[],
  field: GenreSortField,
  direction: SortDirection
): Genre[] {
  const sign = direction === "asc" ? 1 : -1;
  const out = genres.slice();
  out.sort((a, b) => sign * compareGenres(a, b, field));
  return out;
}

// ---------------------------------------------------------------------------
// Persistence (localStorage)
// ---------------------------------------------------------------------------

interface SortState<F extends string> {
  field: F;
  direction: SortDirection;
}

function readState<F extends string>(
  key: string,
  validFields: readonly F[],
  fallback: SortState<F>
): SortState<F> {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as Partial<SortState<F>>;
    const field = (validFields as readonly string[]).includes(parsed.field as string)
      ? (parsed.field as F)
      : fallback.field;
    const direction =
      parsed.direction === "asc" || parsed.direction === "desc"
        ? parsed.direction
        : fallback.direction;
    return { field, direction };
  } catch {
    return fallback;
  }
}

function writeState<F extends string>(key: string, state: SortState<F>): void {
  try {
    localStorage.setItem(key, JSON.stringify(state));
  } catch {
    // best-effort
  }
}

const ALBUM_KEY = "qobee.sort.albums";
const GENRE_KEY = "qobee.sort.genres";
const ARTIST_KEY = "qobee.sort.artists";

const ALBUM_FIELDS: AlbumSortField[] = ALBUM_SORT_OPTIONS.map((o) => o.key);
const GENRE_FIELDS: GenreSortField[] = GENRE_SORT_OPTIONS.map((o) => o.key);
const ARTIST_FIELDS: ArtistSortField[] = ARTIST_SORT_OPTIONS.map((o) => o.key);

export function loadAlbumSort(): SortState<AlbumSortField> {
  return readState(ALBUM_KEY, ALBUM_FIELDS, {
    field: "date_added",
    direction: "desc",
  });
}

export function saveAlbumSort(state: SortState<AlbumSortField>): void {
  writeState(ALBUM_KEY, state);
}

export function loadGenreSort(): SortState<GenreSortField> {
  return readState(GENRE_KEY, GENRE_FIELDS, {
    field: "track_count",
    direction: "desc",
  });
}

export function saveGenreSort(state: SortState<GenreSortField>): void {
  writeState(GENRE_KEY, state);
}

export function loadArtistSort(): SortState<ArtistSortField> {
  return readState(ARTIST_KEY, ARTIST_FIELDS, {
    field: "name",
    direction: "asc",
  });
}

export function saveArtistSort(state: SortState<ArtistSortField>): void {
  writeState(ARTIST_KEY, state);
}
