/**
 * What the app boots into: the window §7 specifies, and the CSP §7 specifies.
 *
 * Both are configuration, so both are asserted by reading
 * `src-tauri/tauri.conf.json` rather than by booting anything. That is the whole
 * of what this file claims. **Two halves stay manual and are not implied
 * covered here**: that the window really opens at that size, and that the
 * running WebView logs no CSP violation. Nothing in this environment boots a
 * WebView (LESSONS, Manual verification item 7).
 *
 * It is still worth a test. A CSP is the kind of setting that is loosened once
 * to unblock something and never tightened again, and the loosening is
 * invisible — the app works better with it. A literal comparison is the only
 * thing that notices.
 */

import { describe, expect, it } from "vitest";

// Imported rather than read off disk: this is the same file Tauri boots from,
// and importing it means a rename or a malformed edit fails the typecheck as
// well as the test. Nothing under `src/` ships this — only this test refers to
// it, so it never reaches the bundle.
import tauriConfig from "../../src-tauri/tauri.conf.json";

/** BUILD-SPEC §7, verbatim. Any change here is a change to the spec. */
const CSP = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:";

const mainWindow = tauriConfig.app.windows[0];

describe("the window the app boots into", () => {
  it("opens wide enough to read a log line in", () => {
    // 800x600 wraps a Monolog header, which is the one line the reader is
    // scanning for. §7 fixes 1100x720.
    expect(mainWindow.width).toBe(1100);
    expect(mainWindow.height).toBe(720);
  });

  it("cannot be dragged down to a size the stream is unreadable at", () => {
    // A minimum has to exist; §7 does not fix the number, so this asserts the
    // constraint rather than the value it currently holds.
    expect(mainWindow.minWidth).toBeGreaterThan(0);
    expect(mainWindow.minHeight).toBeGreaterThan(0);
    expect(mainWindow.minWidth).toBeLessThanOrEqual(mainWindow.width);
    expect(mainWindow.minHeight).toBeLessThanOrEqual(mainWindow.height);
  });
});

describe("the content security policy", () => {
  it("is the policy §7 specifies, character for character", () => {
    // `null` is the scaffold default — no CSP at all — and is what this would
    // silently regress to if the key were ever dropped.
    expect(tauriConfig.app.security.csp).toBe(CSP);
  });

  it("allows no inline script to run", () => {
    // Log text is rendered as text, never as markup, but a policy is the layer
    // that holds when that stops being true. There is no `script-src`, so
    // `default-src` is what governs scripts — and it must not carry the
    // `'unsafe-inline'` that `style-src` legitimately does.
    // Parsed from the *shipped* policy, not from the constant above: a
    // loosened config should fail this on its own terms rather than only
    // through the equality above.
    const directives = new Map(
      tauriConfig.app.security.csp
        .split(";")
        .map((directive) => directive.trim())
        .filter((directive) => directive.length > 0)
        .map((directive) => {
          const [name, ...sources] = directive.split(/\s+/);
          return [name, sources] as const;
        }),
    );

    const scriptSources = directives.get("script-src") ?? directives.get("default-src");
    expect(scriptSources).toBeDefined();
    expect(scriptSources).not.toContain("'unsafe-inline'");
    expect(scriptSources).not.toContain("'unsafe-eval'");
    expect(scriptSources).toContain("'self'");
  });
});
