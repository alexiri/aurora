import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

// P2-UI stage 4 (docs/18 §Dashboard visual design, docs/14 §Visual design "Scope: the whole
// dashboard"): keep every screen on the design tokens, and keep inline styles out of `web/src`
// because the production CSP (`style-src 'self' 'nonce-…'`) blocks `style=` attributes.

const SRC = join(__dirname, "..", "src");

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? walk(p) : [p];
  });
}

const files = walk(SRC).filter((f) => /\.(tsx?|jsx?|css|mdx?)$/.test(f));
const rel = (f: string) => relative(SRC, f).split("\\").join("/");

// Only `globals.css` defines colours; the brand colours live in AuroraMark/AuroraLogo (and its path data).
const COLOUR_EXEMPT = new Set(["app/globals.css", "components/ui/aurora-logo.tsx", "components/ui/aurora-paths.ts"]);

const HUES =
  "neutral|slate|gray|zinc|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose";
const UTILS =
  "bg|text|border|border-[xytblrse]|ring|ring-offset|fill|stroke|from|via|to|divide|outline|decoration|accent|caret|placeholder|shadow";
/** Tailwind palette colour utilities (`bg-red-500`, `dark:text-slate-300/80`, …) and `white`/`black`/`transparent`-less raw colours. */
export const PALETTE = new RegExp(`(?<![\\w-])(?:${UTILS})-(?:(?:${HUES})-\\d{2,3}|white|black)(?![\\w-])`);
/** Hex colour literals (3, 4, 6 or 8 digits) and colour functions in class-name arbitrary values. */
export const HEX = /(?<![\w&])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![\w-])/;
export const COLOUR_FN = /\[(?:rgb|rgba|hsl|hsla|oklch|oklab|lab|lch|color)\(/;
/** A JSX `style=` prop or HTML `style=` attribute. */
export const STYLE_ATTR = /(?<![\w-])style\s*=/;

function offenders(re: RegExp, exempt: Set<string> = new Set()) {
  const out: string[] = [];
  for (const f of files) {
    const r = rel(f);
    if (exempt.has(r)) continue;
    readFileSync(f, "utf8")
      .split("\n")
      .forEach((line, i) => {
        if (re.test(line)) out.push(`${r}:${i + 1}: ${line.trim().slice(0, 120)}`);
      });
  }
  return out;
}

describe("regexes", () => {
  it("flag what they should and nothing else", () => {
    expect(PALETTE.test('className="bg-red-500"')).toBe(true);
    expect(PALETTE.test("hover:text-slate-300 x")).toBe(true);
    expect(PALETTE.test('"border-t-amber-50"')).toBe(true);
    expect(PALETTE.test('"text-white"')).toBe(true);
    expect(PALETTE.test('"bg-danger-ink text-ink-2 bg-accent-soft border-line"')).toBe(false);
    expect(PALETTE.test('"text-on-accent bg-primary"')).toBe(false);
    expect(HEX.test('const c = "#0E1525"')).toBe(true);
    expect(HEX.test('"bg-[#fff]"')).toBe(true);
    expect(HEX.test("&#8212; and &#x2014;")).toBe(false);
    expect(HEX.test('href="#main"')).toBe(false);
    expect(COLOUR_FN.test('"bg-[rgb(1,2,3)]"')).toBe(true);
    expect(STYLE_ATTR.test("<div style={{ width: 1 }} />")).toBe(true);
    expect(STYLE_ATTR.test('<div style="width:1px">')).toBe(true);
    expect(STYLE_ATTR.test("const lifestyle = 1; listStyle")).toBe(false);
    expect(STYLE_ATTR.test("<div className='x' />")).toBe(false);
  });
});

describe("token guard", () => {
  it("scans the dashboard sources", () => {
    expect(files.length).toBeGreaterThan(100);
  });

  it("uses no Tailwind palette colour utility outside globals.css", () => {
    expect(offenders(PALETTE, new Set(["app/globals.css"]))).toEqual([]);
  });

  it("has no hex colour literal outside globals.css and the brand components", () => {
    expect(offenders(HEX, COLOUR_EXEMPT)).toEqual([]);
  });

  it("has no colour function in an arbitrary value outside globals.css", () => {
    expect(offenders(COLOUR_FN, new Set(["app/globals.css"]))).toEqual([]);
  });
});

describe("inline-style guard", () => {
  it("has no style= prop or attribute anywhere in web/src (the production CSP blocks it)", () => {
    expect(offenders(STYLE_ATTR)).toEqual([]);
  });
});
