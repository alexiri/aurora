import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// jsdom + a loaded host make the query -> render chains slow; the 1s testing-library default caused
// random timeouts on the full run (the per-test budget is already 20s, see vitest.config.ts).
// 3s still flaked at host load ~48 (FIX-067): a wait only costs its full timeout when the awaited
// condition is wrong, so a long bound is free on green runs and stays far below the 20s test budget.
// Tests must wait with testing-library's findBy*/waitFor (which honour this), not `vi.waitFor` (fixed 1s).
configure({ asyncUtilTimeout: 10000 });
