import { defineConfig } from "vitest/config";

/**
 * Vitest runs the stream hook, the filter derivation, the pure view helpers —
 * and, since D10 was narrowed, two rendered components: `ProjectRow` (an
 * unhealthy **Project**'s inline reason, D4) and the **Activity** badge on it
 * (D8). Those two behaviours have no other way to be checked. `ProjectList` is
 * rendered in one test only, as the container needed to show that an unhealthy
 * Project is still *in* the list — the half of D4 a single row cannot show.
 * Nothing else is rendered, so `jsdom` is still a host rather than a subject.
 *
 * What it deliberately cannot do: **layout**. jsdom reports `offsetTop`,
 * `offsetHeight`, `scrollHeight` and `clientHeight` as 0, so anything about
 * scroll position is tested as arithmetic (`scrollAnchor.ts`, `streamWindow.ts`)
 * and confirmed in pixels by a human. See BUILD-SPEC §10.
 */
export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
