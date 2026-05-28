/**
 * Property tests for the EQ display formatters.
 *
 * Tag: Property 3 — Round-trip des formatters EQ
 * Feature: native-media-integration-and-ux
 *
 * Validates: Requirements R4.1, R4.2, R4.3, R4.4; Property 3
 *
 * `formatGainDb` MUST be a lossless string projection of the gain
 * value: parsing the produced string back to a number recovers the
 * original (within floating-point tolerance) and the explicit sign
 * prefix matches the sign of the input. `formatFrequency` MUST use
 * Hz below 1 kHz and kHz at or above 1 kHz on every canonical band.
 */
import { describe, expect, it } from "vitest";
import fc from "fast-check";

import { formatFrequency, formatGainDb } from "../format";

// ---------------------------------------------------------------------------
// Helpers — local to the test file (kept out of the production surface).
// ---------------------------------------------------------------------------

/**
 * Extract the numeric portion of a `formatGainDb` output.
 *
 * The formatter produces strings of the form `<signed-number> dB`,
 * where `<signed-number>` is whatever `String(v)` returns for the
 * input gain. That includes scientific notation for very small or
 * very large floats (e.g. `"+1.4e-45 dB"`), so we strip the `" dB"`
 * suffix and let `Number()` handle the lexical form natively.
 *
 * Examples:
 *   parseFormat("0 dB")          === 0
 *   parseFormat("+1.5 dB")       === 1.5
 *   parseFormat("-12 dB")        === -12
 *   parseFormat("+1.4e-45 dB")   === 1.4e-45
 */
function parseFormat(s: string): number {
  const SUFFIX = " dB";
  if (!s.endsWith(SUFFIX)) {
    throw new Error(`unparseable gain string: ${JSON.stringify(s)}`);
  }
  const head = s.slice(0, -SUFFIX.length);
  const n = Number(head);
  if (Number.isNaN(n)) {
    throw new Error(`unparseable gain string: ${JSON.stringify(s)}`);
  }
  return n;
}

/** `"+"` if the formatted string starts with `+`, `"-"` if it starts with `-`, `""` otherwise. */
function signPrefix(s: string): "+" | "-" | "" {
  if (s.startsWith("+")) return "+";
  if (s.startsWith("-")) return "-";
  return "";
}

/** Sign of a numeric gain value. */
function sign(v: number): "+" | "-" | "" {
  if (v > 0) return "+";
  if (v < 0) return "-";
  return "";
}

// ---------------------------------------------------------------------------
// Property 3 — round-trip formatGainDb on the audible EQ range.
// ---------------------------------------------------------------------------

describe("Property 3: formatGainDb round-trip", () => {
  it("parseFormat ∘ formatGainDb === id and signPrefix matches sign", () => {
    fc.assert(
      fc.property(
        fc.float({ min: -12, max: 12, noNaN: true, noDefaultInfinity: true }),
        (v: number) => {
          const formatted = formatGainDb(v);
          const parsed = parseFormat(formatted);
          // `formatGainDb` is a lossless string projection of `v`
          // (it's `String(v)` modulo an explicit `+` for positives),
          // so the round-trip is bit-exact for every finite JS number.
          // We use `===` (not `Object.is`) so the +0/-0 case — for
          // which the formatter intentionally collapses to `"0 dB"`
          // — still satisfies the round-trip.
          expect(parsed === v).toBe(true);
          // Sign projection: must match the actual sign of v.
          expect(signPrefix(formatted)).toBe(sign(v));
        },
      ),
      { numRuns: 100 },
    );
  });
});

// ---------------------------------------------------------------------------
// Property 3 (suite) — exhaustive table for the 10 canonical EQ bands.
// ---------------------------------------------------------------------------

const CANONICAL_BANDS_HZ: ReadonlyArray<number> = [
  32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000,
];

describe("Property 3: formatFrequency exhaustive table", () => {
  for (const hz of CANONICAL_BANDS_HZ) {
    it(`uses ${hz < 1000 ? "Hz" : "kHz"} for ${hz} Hz`, () => {
      const out = formatFrequency(hz);
      const expectedUnit = hz < 1000 ? "Hz" : "kHz";
      const otherUnit = expectedUnit === "Hz" ? "kHz" : "Hz";

      // Unit selection: Hz iff hz < 1000, kHz otherwise.
      expect(out.endsWith(` ${expectedUnit}`)).toBe(true);
      expect(out.endsWith(` ${otherUnit}`)).toBe(false);

      // Numeric portion parses to the right value.
      const match = out.match(/^(\d+(?:\.\d+)?)\s*(Hz|kHz)$/);
      expect(match).not.toBeNull();
      const [, numberPart, unit] = match!;
      const parsed = Number(numberPart);
      const recovered = unit === "Hz" ? parsed : parsed * 1000;
      expect(recovered).toBe(hz);
    });
  }
});

// ---------------------------------------------------------------------------
// Hand-picked sign branches — sanity check for the three formatGainDb cases.
// ---------------------------------------------------------------------------

describe("formatGainDb explicit sign branches", () => {
  it("renders a positive gain with an explicit `+` prefix", () => {
    expect(formatGainDb(3)).toBe("+3 dB");
    expect(formatGainDb(0.5)).toBe("+0.5 dB");
    expect(formatGainDb(12)).toBe("+12 dB");
  });

  it("renders zero without any sign prefix", () => {
    expect(formatGainDb(0)).toBe("0 dB");
  });

  it("renders a negative gain with an explicit `-` prefix and absolute value", () => {
    expect(formatGainDb(-1)).toBe("-1 dB");
    expect(formatGainDb(-4.5)).toBe("-4.5 dB");
    expect(formatGainDb(-12)).toBe("-12 dB");
  });
});
