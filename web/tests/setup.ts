import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// jsdom + a loaded host make the query -> render chains slow; the 1s testing-library default caused
// random timeouts on the full run (the per-test budget is already 20s, see vitest.config.ts).
configure({ asyncUtilTimeout: 3000 });
