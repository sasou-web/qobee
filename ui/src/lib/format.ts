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

/**
 * EQ gain label.
 *
 * - `v > 0`  → `"+{v} dB"` (explicit `+`)
 * - `v === 0`→ `"0 dB"` (no sign)
 * - `v < 0`  → `"-{|v|} dB"` (explicit `-`)
 *
 * The numeric portion is stringified as-is so a half-step like `1.5`
 * renders as `"+1.5 dB"`. Callers that want a fixed precision should
 * round before invoking this function.
 *
 * Validates: Requirements R4.1, R4.2, R4.3.
 */
export function formatGainDb(v: number): string {
  if (v > 0) return `+${v} dB`;
  if (v === 0) return `0 dB`;
  return `-${Math.abs(v)} dB`;
}

/**
 * EQ band frequency label.
 *
 * Below 1000 Hz the value is shown in Hz (`"32 Hz"`), at or above
 * 1000 Hz it switches to kHz (`"1 kHz"`, `"16 kHz"`). Division is a
 * plain `/ 1000` so canonical bands like 1000 / 2000 / 4000 / 8000
 * / 16000 produce integer kHz strings.
 *
 * Validates: Requirements R4.4.
 */
export function formatFrequency(hz: number): string {
  return hz < 1000 ? `${hz} Hz` : `${hz / 1000} kHz`;
}
