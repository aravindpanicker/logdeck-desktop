/**
 * Splitting text into matched and unmatched runs, for search highlighting.
 *
 * Pure and JSX-free so it can be unit-tested directly: this is the only part of
 * the reveal in D7 that has interesting logic, and getting it wrong corrupts the
 * text of an Entry on screen rather than merely mis-colouring it.
 *
 * Matching is deliberately the same shape as `matchesQuery` in `lib/filter.ts` —
 * trimmed, case-insensitive, literal substring. The two must agree: an Entry
 * that the filter kept because it matched, but that highlights nothing, reads as
 * a bug in the search box.
 */

/** One stretch of text, either inside a hit or between hits. */
export interface HighlightRun {
  readonly text: string;
  /** True when this run is one occurrence of the query. */
  readonly isMatch: boolean;
}

/** The needle as it is actually searched for. Blank means "no query". */
export function normaliseQuery(query: string): string {
  return query.trim().toLowerCase();
}

/**
 * Case folding can change a string's length — `"İ".toLowerCase()` is two code
 * units — which would slide every index after it and slice the text apart in the
 * wrong place. When that happens we search the original text instead: the hit
 * count may be lower, but no run is ever mis-sliced.
 */
function foldedHaystack(text: string): { haystack: string; needleFolds: boolean } {
  const lowered = text.toLowerCase();
  return lowered.length === text.length
    ? { haystack: lowered, needleFolds: true }
    : { haystack: text, needleFolds: false };
}

function scan(
  text: string,
  query: string,
  onHit: (start: number, length: number) => void,
): void {
  const folded = normaliseQuery(query);
  if (folded === "" || text === "") return;

  const { haystack, needleFolds } = foldedHaystack(text);
  const needle = needleFolds ? folded : query.trim();
  if (needle === "") return;

  let cursor = 0;
  for (;;) {
    const hit = haystack.indexOf(needle, cursor);
    if (hit === -1) return;
    onHit(hit, needle.length);
    // Occurrences never overlap: "aa" appears twice in "aaaa", not three times,
    // which is what a reader counting highlighted spans on screen will see.
    cursor = hit + needle.length;
  }
}

/**
 * `text` split into consecutive runs, in order.
 *
 * Concatenating every run's `text` always reproduces `text` exactly — the runs
 * are a partition, not a transformation, so highlighting can never alter what an
 * Entry says.
 */
export function splitHighlights(text: string, query: string): HighlightRun[] {
  if (text === "") return [];

  const runs: HighlightRun[] = [];
  let cursor = 0;

  scan(text, query, (start, length) => {
    if (start > cursor) {
      runs.push({ text: text.slice(cursor, start), isMatch: false });
    }
    runs.push({ text: text.slice(start, start + length), isMatch: true });
    cursor = start + length;
  });

  if (cursor < text.length) {
    runs.push({ text: text.slice(cursor), isMatch: false });
  }
  return runs;
}

/**
 * How many times the query occurs in `text`.
 *
 * Counted without building the runs, because this is called on the *collapsed*
 * context of every rendered Entry to decide whether it must reveal itself (D7),
 * and the runs of a 47-frame trace are only worth allocating once it does.
 */
export function countMatches(text: string, query: string): number {
  let total = 0;
  scan(text, query, () => {
    total += 1;
  });
  return total;
}
