/**
 * `highlight.ts` is the only pure logic in the stream UI, and the one place a
 * rendering bug could alter the text of an Entry rather than merely its colour.
 * The partition invariant below is the assertion that matters most.
 */

import { describe, expect, it } from "vitest";

import { matchesQuery } from "./filter";
import {
  countMatches,
  normaliseQuery,
  splitHighlights,
  type HighlightRun,
} from "./highlight";

const TRACE = [
  "[2026-08-14 01:28:00] local.ERROR: Undefined variable $user",
  '{"exception":"[object] (ErrorException(code: 0))"}',
  "[stacktrace]",
  "#0 /var/www/app/Http/Controllers/UserController.php(42): handle()",
  "#1 /var/www/vendor/laravel/framework/src/Illuminate/Routing/Route.php(260)",
].join("\n");

function rejoin(runs: readonly HighlightRun[]): string {
  return runs.map((run) => run.text).join("");
}

function matched(runs: readonly HighlightRun[]): string[] {
  return runs.filter((run) => run.isMatch).map((run) => run.text);
}

describe("normaliseQuery", () => {
  it("trims and folds case so the needle matches filter.ts", () => {
    expect(normaliseQuery("  UserController \n")).toBe("usercontroller");
  });

  it("reports a whitespace-only query as no query", () => {
    expect(normaliseQuery("   ")).toBe("");
  });
});

describe("splitHighlights", () => {
  it("returns the whole text as one unmatched run when there is no query", () => {
    expect(splitHighlights("Something failed", "")).toEqual([
      { text: "Something failed", isMatch: false },
    ]);
  });

  it("returns no runs for empty text", () => {
    expect(splitHighlights("", "user")).toEqual([]);
    expect(splitHighlights("", "")).toEqual([]);
  });

  it("treats a whitespace-only query as no query", () => {
    expect(splitHighlights("abc", "   ")).toEqual([
      { text: "abc", isMatch: false },
    ]);
  });

  it("splits a single hit into before, hit, and after", () => {
    expect(splitHighlights("the user failed", "user")).toEqual([
      { text: "the ", isMatch: false },
      { text: "user", isMatch: true },
      { text: " failed", isMatch: false },
    ]);
  });

  it("matches case-insensitively but preserves the original casing", () => {
    const runs = splitHighlights("UserController", "usercontroller");
    expect(matched(runs)).toEqual(["UserController"]);
  });

  it("emits no empty run when the hit is at the start or the end", () => {
    const runs = splitHighlights("user", "user");
    expect(runs).toEqual([{ text: "user", isMatch: true }]);
    expect(runs.every((run) => run.text !== "")).toBe(true);
  });

  it("finds every occurrence", () => {
    const runs = splitHighlights("a-b-a-b-a", "a");
    expect(matched(runs)).toEqual(["a", "a", "a"]);
  });

  it("counts overlapping candidates once, left to right", () => {
    expect(matched(splitHighlights("aaaa", "aa"))).toEqual(["aa", "aa"]);
  });

  it("does not re-count a candidate that overlaps the hit before it", () => {
    // "aaa" is the case "aaaa" cannot pin: a scanner advancing by one instead of
    // by the needle's length would report hits at 0 *and* 1 here, and the second
    // run would be sliced out of text the first run already claimed.
    const runs = splitHighlights("aaa", "aa");
    expect(matched(runs)).toEqual(["aa"]);
    expect(runs).toEqual([
      { text: "aa", isMatch: true },
      { text: "a", isMatch: false },
    ]);
    expect(rejoin(runs)).toBe("aaa");
    expect(countMatches("aaa", "aa")).toBe(1);
    expect(countMatches("abababa", "aba")).toBe(2);
  });

  it("treats regex metacharacters as literal text", () => {
    const runs = splitHighlights("cost is $user.*", "$user.*");
    expect(matched(runs)).toEqual(["$user.*"]);
    expect(matched(splitHighlights("xyz", ".*"))).toEqual([]);
  });

  it("spans newlines, so a hit inside a stack frame is found", () => {
    const runs = splitHighlights(TRACE, "UserController.php");
    expect(matched(runs)).toEqual(["UserController.php"]);
    expect(rejoin(runs)).toBe(TRACE);
  });

  it("partitions the text — rejoining the runs reproduces it exactly", () => {
    for (const query of ["", " ", "e", "php", "[stacktrace]", "\n#", "zzz"]) {
      expect(rejoin(splitHighlights(TRACE, query))).toBe(TRACE);
    }
  });

  it("does not mis-slice text whose case folding changes length", () => {
    // "İ".toLowerCase() is two code units; sliced on folded indices this would
    // cut the following characters apart.
    const text = "İstanbul deploy İstanbul";
    const runs = splitHighlights(text, "İstanbul");
    expect(rejoin(runs)).toBe(text);
    expect(matched(runs)).toEqual(["İstanbul", "İstanbul"]);
  });

  /*
   * `matchesQuery` and this module normalise the needle independently. Nothing
   * in the type system ties them together, so the agreement is pinned here
   * instead: if either side's trimming or case folding drifts, the corpus below
   * produces an Entry the toolbar keeps but that highlights nothing — the exact
   * "reads as a bug in the search box" failure.
   */
  it("agrees with the filter across a corpus of queries", () => {
    const queries = [
      "errorexception",
      "ERRORexception",
      "  UserController  ",
      "$user",
      ".*",
      "#0",
      "/var/www",
      "\n#1",
      "İstanbul",
      "zzz",
      "",
      "   ",
    ];

    for (const query of queries) {
      const hits = matched(splitHighlights(TRACE, query)).length;
      if (normaliseQuery(query) === "") {
        // A blank query passes every Entry and highlights nothing; that is not
        // a disagreement, it is the absence of a search.
        expect(matchesQuery(TRACE, query)).toBe(true);
        expect(hits).toBe(0);
        continue;
      }
      expect(hits > 0).toBe(matchesQuery(TRACE, query));
    }
  });
});

describe("countMatches", () => {
  it("is zero without a query", () => {
    expect(countMatches(TRACE, "")).toBe(0);
    expect(countMatches(TRACE, "  ")).toBe(0);
  });

  it("is zero for empty text", () => {
    expect(countMatches("", "user")).toBe(0);
  });

  it("counts every non-overlapping occurrence", () => {
    expect(countMatches("aaaa", "aa")).toBe(2);
    expect(countMatches("a-b-a-b-a", "a")).toBe(3);
  });

  it("counts hits across lines of a trace", () => {
    expect(countMatches(TRACE, "/var/www")).toBe(2);
  });

  it("agrees with splitHighlights", () => {
    for (const query of ["php", "#", "user", "zzz"]) {
      expect(countMatches(TRACE, query)).toBe(
        matched(splitHighlights(TRACE, query)).length,
      );
    }
  });
});
