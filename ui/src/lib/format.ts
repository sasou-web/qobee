// Display formatting helpers used by the UI components.

/** "3:42", "1:02:08" — accepts seconds (may be fractional). */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  }
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** "44.1 kHz / 16-bit" or "—" when unknown. */
export function formatQuality(
  sampleRate: number | null,
  bitDepth: number | null
): string {
  const parts: string[] = [];
  if (sampleRate !== null) {
    const khz = sampleRate / 1000;
    parts.push(
      khz % 1 === 0 ? `${khz.toFixed(0)} kHz` : `${khz.toFixed(1)} kHz`
    );
  }
  if (bitDepth !== null) parts.push(`${bitDepth}-bit`);
  return parts.length === 0 ? "—" : parts.join(" / ");
}

/** "WASAPI (Shared)" — names the actual Windows API in the path. */
export function formatOutputMode(mode: string): string {
  switch (mode) {
    case "shared":
      return "WASAPI (Shared)";
    default:
      return mode;
  }
}
