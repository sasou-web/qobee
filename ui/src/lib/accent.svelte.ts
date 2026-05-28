// Dynamic accent color driven by the currently playing cover.
//
// Whenever the active track changes, we extract the cover's dominant
// color, snap it onto the app's accent CSS variables, and let every
// component that reads `--accent` pick up the new tint. The user's
// configured accent (Settings → Appearance → Accent color) acts as a
// fallback for tracks without artwork or before the palette finishes
// loading.

import { coverUrl } from "./api";
import { extractPalette, type Swatch } from "./palette";

/** Mix two channels by `t` (0..1). */
function lerp(a: number, b: number, t: number): number {
  return Math.round(a + (b - a) * t);
}

/** Pick the most "vibrant" swatch — high saturation, mid lightness —
 *  rather than just the most populous one. Black/white covers tend to
 *  produce massive grayscale buckets that would otherwise win. */
function pickVibrant(palette: Swatch[]): Swatch | null {
  if (palette.length === 0) return null;
  let best: Swatch | null = null;
  let bestScore = -Infinity;
  for (const s of palette) {
    const max = Math.max(s.r, s.g, s.b);
    const min = Math.min(s.r, s.g, s.b);
    const sat = max === 0 ? 0 : (max - min) / max;
    const lum = (0.2126 * s.r + 0.7152 * s.g + 0.0722 * s.b) / 255;
    // Prefer saturated colors that aren't too dark or too washed out.
    // The 0.5 - |lum - 0.55| term peaks around medium-bright colors.
    const score = sat * 1.4 + (0.5 - Math.abs(lum - 0.55));
    if (score > bestScore) {
      bestScore = score;
      best = s;
    }
  }
  return best;
}

function rgb(s: Swatch): string {
  return `rgb(${s.r}, ${s.g}, ${s.b})`;
}

function rgba(s: Swatch, a: number): string {
  return `rgba(${s.r}, ${s.g}, ${s.b}, ${a})`;
}

/** Lighten a swatch towards white by `t` (0..1). */
function lighten(s: Swatch, t: number): Swatch {
  return {
    r: lerp(s.r, 255, t),
    g: lerp(s.g, 255, t),
    b: lerp(s.b, 255, t),
    population: s.population,
  };
}

/** Relative luminance of a swatch (Rec. 709), 0..1. */
function luminance(s: Swatch): number {
  return (0.2126 * s.r + 0.7152 * s.g + 0.0722 * s.b) / 255;
}

/**
 * Make sure the swatch is bright enough to read against the dark UI.
 * A near-black album cover (e.g. moody hip-hop artwork) used to push
 * the accent into the `rgb(20, 20, 25)` range, which made the
 * accent-tinted PlayButton on AlbumDetail look like a black disc with
 * a faint glow halo.
 *
 * If the picked swatch is below `MIN_LUM`, we lighten it towards
 * white until it crosses the threshold. The hue is preserved, only
 * the brightness is lifted.
 */
function ensureLegible(s: Swatch): Swatch {
  const MIN_LUM = 0.42;
  let cur = s;
  let guard = 0;
  while (luminance(cur) < MIN_LUM && guard < 10) {
    cur = lighten(cur, 0.25);
    guard++;
  }
  return cur;
}

class AccentStore {
  /** When true, the accent follows the currently playing cover. When
   *  false, we revert to the user's configured accent on the next
   *  track change. */
  enabled = $state<boolean>(true);

  /** Active swatch, or null when no cover is in flight. Exposed so
   *  components (e.g. PlayerBar background gradient) can read it
   *  directly instead of through CSS. */
  current = $state<Swatch | null>(null);

  private root: HTMLElement | null = null;
  private lastKey: string | null = null;
  private busy = false;

  bind(root: HTMLElement | null): void {
    this.root = root;
  }

  setEnabled(on: boolean): void {
    this.enabled = on;
    if (!on) this.reset();
  }

  reset(): void {
    if (!this.root) return;
    this.root.style.removeProperty("--accent");
    this.root.style.removeProperty("--accent-soft");
    this.root.style.removeProperty("--accent-glow");
    this.lastKey = null;
    this.current = null;
  }

  /** Update the accent for the given cover key. Skips redundant calls
   *  when the cover hasn't changed. */
  async update(coverKey: string | null): Promise<void> {
    if (!this.enabled || !this.root) return;
    if (this.busy) return;
    if (!coverKey) {
      this.reset();
      return;
    }
    if (coverKey === this.lastKey) return;
    this.lastKey = coverKey;
    const url = coverUrl(coverKey);
    if (!url) return;
    this.busy = true;
    try {
      const palette = await extractPalette(url, 5);
      const raw = pickVibrant(palette);
      if (!raw) {
        this.reset();
        return;
      }
      const pick = ensureLegible(raw);
      this.current = pick;
      const root = this.root;
      if (!root) return;
      root.style.setProperty("--accent", rgb(pick));
      root.style.setProperty("--accent-soft", rgba(pick, 0.18));
      // The glow is the same color, deeper saturation, more spread.
      root.style.setProperty("--accent-glow", rgba(lighten(pick, 0.05), 0.45));
    } catch {
      this.reset();
    } finally {
      this.busy = false;
    }
  }
}

export const accent = new AccentStore();
