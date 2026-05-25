// Tiny client-side palette extractor.
//
// Loads a cover URL into an offscreen canvas, samples its pixels, and
// runs a small bucket-based clustering to surface the dominant colors.
// We don't pull in a heavy library because the input set is small (one
// album cover at a time) and we only need a handful of swatches.

/** RGB triplet in `[0, 255]`. */
export interface Swatch {
  r: number;
  g: number;
  b: number;
  /** How many sampled pixels mapped to this swatch's bucket. */
  population: number;
}

/** Convert a swatch to a CSS color string. */
export function swatchToCss(s: Swatch): string {
  return `rgb(${s.r}, ${s.g}, ${s.b})`;
}

/** Pick a readable foreground color (black or white) for a swatch. */
export function swatchTextColor(s: Swatch): string {
  // Relative luminance per WCAG 2; threshold tuned for contrast on
  // medium swatches. Pure white text on bright pastels would otherwise
  // wash out.
  const lum =
    (0.2126 * s.r + 0.7152 * s.g + 0.0722 * s.b) / 255;
  return lum > 0.55 ? "rgb(20, 22, 26)" : "rgb(245, 245, 247)";
}

/**
 * Extract up to `count` dominant colors from `imageUrl`.
 *
 * The image is downscaled to a small thumbnail (max edge ~96px) so
 * sampling stays cheap. Colors are bucketed in a 5-bit-per-channel
 * grid (32³ buckets), then the top buckets are returned with a small
 * "merge close colors" pass to avoid near-duplicates dominating.
 */
export async function extractPalette(
  imageUrl: string,
  count = 5
): Promise<Swatch[]> {
  const img = await loadImage(imageUrl);

  // Downscale onto an offscreen canvas. Reading every original pixel
  // would be wasteful — covers can be 1000×1000 or larger — and a
  // small thumbnail still preserves the overall color story.
  const maxEdge = 96;
  const scale = Math.min(1, maxEdge / Math.max(img.width, img.height));
  const w = Math.max(1, Math.round(img.width * scale));
  const h = Math.max(1, Math.round(img.height * scale));

  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("2d context unavailable");
  ctx.drawImage(img, 0, 0, w, h);

  let data: Uint8ClampedArray;
  try {
    data = ctx.getImageData(0, 0, w, h).data;
  } catch (e) {
    // Cross-origin taint can throw on `getImageData`. The qobee-cover
    // protocol is same-origin, but stay defensive.
    throw new Error(`palette extraction failed: ${String(e)}`);
  }

  // Bucket pixels by 5-bit-per-channel quantization. 32^3 = 32768
  // buckets at most; in practice covers fill only a few hundred.
  const buckets = new Map<number, { r: number; g: number; b: number; n: number }>();
  for (let i = 0; i < data.length; i += 4) {
    const r = data[i] ?? 0;
    const g = data[i + 1] ?? 0;
    const b = data[i + 2] ?? 0;
    const a = data[i + 3] ?? 0;
    if (a < 64) continue; // mostly-transparent pixels skew nothing
    // Skip near-black / near-white extremes so the palette reflects
    // *colors* rather than the artwork's matte/letterbox borders.
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    if (max < 16 || min > 240) continue;

    const key = ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3);
    const cur = buckets.get(key);
    if (cur) {
      cur.r += r;
      cur.g += g;
      cur.b += b;
      cur.n++;
    } else {
      buckets.set(key, { r, g, b, n: 1 });
    }
  }

  // Convert buckets to swatches sorted by population.
  const all: Swatch[] = Array.from(buckets.values())
    .map((b) => ({
      r: Math.round(b.r / b.n),
      g: Math.round(b.g / b.n),
      b: Math.round(b.b / b.n),
      population: b.n,
    }))
    .sort((a, b) => b.population - a.population);

  // Greedy merge: walk the top swatches and skip any that's too close
  // to one we already kept. Distance is sum of channel deltas — cheap
  // and good enough for "don't show me five shades of beige".
  const minDist = 60;
  const picked: Swatch[] = [];
  for (const s of all) {
    if (picked.length >= count) break;
    const close = picked.some(
      (p) => Math.abs(p.r - s.r) + Math.abs(p.g - s.g) + Math.abs(p.b - s.b) < minDist
    );
    if (!close) picked.push(s);
  }

  // Fallbacks: if we couldn't find `count` distinct colors, top up
  // with the leading buckets even when close — better to render five
  // similar swatches than three blanks.
  while (picked.length < count && all.length > picked.length) {
    const next = all[picked.length];
    if (next) picked.push(next);
    else break;
  }

  return picked;
}

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    // Cover URLs come from the `qobee-cover://` custom protocol which
    // is same-origin to the webview, so crossOrigin isn't strictly
    // required — but setting it makes intent explicit.
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = (e) => reject(e instanceof Error ? e : new Error("image load failed"));
    img.src = url;
  });
}
