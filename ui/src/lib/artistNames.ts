// Split an "artist" tag value into individual artist names.
//
// Music files commonly bundle multiple credits in a single string
// ("Foo, Bar", "Foo & Bar", "Foo feat. Bar", "Foo x Bar", "Foo / Bar"…)
// because most tag formats only carry one ARTIST field. Splitting them
// lets the UI surface each contributor as its own clickable link.
//
// We try to be conservative: ambiguous separators (a comma without a
// trailing space, a slash inside what could be a stylized name) are
// left alone. If a split produces no entries the original string is
// returned unchanged so the row never goes blank.

const SEPARATORS = [
  // Order matters: longer matches first so " featuring " beats " feat. ".
  / featuring /i,
  / feat\.? /i,
  / ft\.? /i,
  / with /i,
  / vs\.? /i,
  / x /i,
  /, & /,
  /, /,
  / & /,
  / and /i,
  / \+ /,
  / \/ /,
  /; /,
];

export function splitArtists(raw: string): string[] {
  const trimmed = raw.trim();
  if (!trimmed) return [];

  // Build a regex that matches any of the separators in one pass.
  const combined = new RegExp(
    SEPARATORS.map((r) => r.source).join("|"),
    "i"
  );

  const parts = trimmed
    .split(combined)
    .map((p) => p.trim())
    .filter((p) => p.length > 0);

  if (parts.length <= 1) {
    // Nothing to split — return the input as-is.
    return [trimmed];
  }
  // De-duplicate while preserving order. Same artist sometimes shows
  // up twice in messy tags ("Foo, Foo & Bar").
  const seen = new Set<string>();
  const out: string[] = [];
  for (const p of parts) {
    const key = p.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(p);
  }
  return out;
}
