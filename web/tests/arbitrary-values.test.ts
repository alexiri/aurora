import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

// docs/14 §Visual design: tokens only, no one-off values. Arbitrary Tailwind values (`max-w-[1160px]`,
// `tracking-[0.06em]`, `w-[min(40rem,…)]`) are replaced by theme tokens (`max-w-page`, `tracking-label`), by a
// `size` prop (`DialogContent`), or by the shared primitives. The few that remain are listed here, each with the
// reason it cannot be a token. Adding to this list needs a reason a reviewer accepts.

const ROOTS = ["src/components", "src/app"];

/** `[12px]`, `[1.5rem]`, `[80%]`, `[#fff]` and the viewport-arithmetic functions. */
const ARBITRARY = /\[(#|[0-9.]+(px|rem|em|%)|(min|max|calc|clamp)\()/;

/** file (relative to `web/`) → [substring of the offending line, reason]. */
const ALLOWED: Record<string, [string, string][]> = {
  "src/components/ui/dialog.tsx": [
    ["max-h-[calc(100dvh-2rem)]", "the dialog never exceeds the viewport minus a 1 rem gutter"],
    ["w-[min(", "the `size` map: each width is capped at the viewport minus a 1 rem gutter (the only place)"],
  ],
  "src/components/chat/chat-panel.tsx": [
    ["h-[calc(100dvh-", "the chat panel fills the viewport below the page header (docs/14: the page itself does not scroll)"],
    ["min-h-[560px]", "the chat panel's minimum height (docs/14 agent page: at least 560 px)"],
  ],
  "src/components/app-shell.tsx": [
    ["w-[min(18rem,calc(100vw-3rem))]", "the mobile navigation drawer leaves a 3 rem strip of page visible"],
  ],
};

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? files(p) : /\.(tsx|ts)$/.test(name) ? [p] : [];
  });
}

describe("no arbitrary Tailwind values outside the allow-list", () => {
  const root = process.cwd();
  const found: { file: string; line: number; text: string }[] = [];
  for (const r of ROOTS) {
    for (const f of files(join(root, r))) {
      readFileSync(f, "utf8")
        .split("\n")
        .forEach((text, i) => {
          if (ARBITRARY.test(text)) found.push({ file: relative(root, f), line: i + 1, text: text.trim() });
        });
    }
  }

  it("every arbitrary value is on the allow-list", () => {
    const unlisted = found.filter(
      (f) => !(ALLOWED[f.file] ?? []).some(([needle]) => f.text.includes(needle)),
    );
    expect(unlisted.map((f) => `${f.file}:${f.line}  ${f.text.slice(0, 140)}`)).toEqual([]);
  });

  it("the allow-list has no stale entries", () => {
    const stale = Object.entries(ALLOWED).flatMap(([file, entries]) =>
      entries
        .filter(([needle]) => !found.some((f) => f.file === file && f.text.includes(needle)))
        .map(([needle]) => `${file}: ${needle}`),
    );
    expect(stale).toEqual([]);
  });
});
