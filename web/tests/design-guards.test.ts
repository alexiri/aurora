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

// (FIX-149, D-272) A view on top of a page is a `Dialog`, never a dashboard page in another browser window. The only
// `window.open` calls are the OAuth provider's sign-in window and the terminal's owner-requested "Open in new window"
// (FIX-138: a full standalone terminal page in a new tab, `noopener`); `window.opener` and `window.close` appear nowhere.
describe("window guard", () => {
  const WINDOW_OPEN_ALLOWED = new Set(["lib/api/apps.ts", "components/agents/terminal-tab.tsx"]);
  it("opens windows only in the sign-in helper and the terminal's Open in new window", () => {
    expect(offenders(/\bwindow\.open\s*\(/, WINDOW_OPEN_ALLOWED)).toEqual([]);
  });
  it("keeps each allowed call to exactly one call site", () => {
    const count = (path: string) => (readFileSync(join(SRC, path), "utf8").match(/\bwindow\.open\s*\(/g) ?? []).length;
    expect(count("lib/api/apps.ts")).toBe(1);
  });
  it("never refers to window.opener or window.close", () => {
    expect(offenders(/\bwindow\.(opener|close)\b/)).toEqual([]);
  });
});

describe("inline-style guard", () => {
  it("has no style= prop or attribute anywhere in web/src (the production CSP blocks it)", () => {
    expect(offenders(STYLE_ATTR)).toEqual([]);
  });
});

// (P5-POLISH-1, D-393) Unlayered CSS beats Tailwind's layered utilities: an unlayered `a { color: inherit }` blanked every
// `text-on-primary` link. Element selectors belong in `@layer base`.
/** Top-level selectors of a stylesheet that are outside `@layer`/`@theme`/`@utility` and are not a class, id, attribute or `:root`. */
export function unlayeredElementSelectors(css: string): string[] {
  const src = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const out: string[] = [];
  const TRANSPARENT = /^@(media|supports|container)\b/;
  let i = 0;
  const walkBlock = (): void => {
    while (i < src.length) {
      const open = src.indexOf("{", i);
      const semi = src.indexOf(";", i);
      const close = src.indexOf("}", i);
      if (close !== -1 && (open === -1 || close < open) && (semi === -1 || close < semi)) {
        i = close + 1;
        return;
      }
      if (semi !== -1 && (open === -1 || semi < open)) {
        i = semi + 1; // a declaration or `@import …;`
        continue;
      }
      if (open === -1) return;
      const prelude = src.slice(i, open).trim();
      i = open + 1;
      if (prelude.startsWith("@")) {
        if (TRANSPARENT.test(prelude)) walkBlock();
        else skipBlock();
      } else {
        for (const sel of prelude.split(",")) {
          const s = sel.trim();
          if (s && !/^(\.|#|\[|:root)/.test(s)) out.push(s);
        }
        skipBlock();
      }
    }
  };
  const skipBlock = (): void => {
    let depth = 1;
    while (i < src.length && depth > 0) {
      if (src[i] === "{") depth++;
      else if (src[i] === "}") depth--;
      i++;
    }
  };
  walkBlock();
  return out;
}

/** `focus:` utilities (a mouse click would leave a ring): only `focus-visible:` / `focus-within:` are allowed. */
export const FOCUS_UTILITY = /(?<![\w-])focus:[a-z!\[-]/;
/** A `:focus` pseudo-class without `-visible` / `-within` in CSS. */
export const FOCUS_PSEUDO = /:focus(?![-\w])/;

describe("layer guard", () => {
  it("recognises an unlayered element rule", () => {
    expect(unlayeredElementSelectors("a { color: inherit; }")).toEqual(["a"]);
    expect(unlayeredElementSelectors("@layer base { a { color: inherit; } } .x { a: b } :root { --a: 1 }")).toEqual([]);
    expect(unlayeredElementSelectors("@media (max-width: 1px) { body { x: y } }")).toEqual(["body"]);
    expect(FOCUS_UTILITY.test('"focus:ring-2"')).toBe(true);
    expect(FOCUS_UTILITY.test('"focus-visible:ring-2 has-[:focus-visible]:outline"')).toBe(false);
    expect(FOCUS_PSEUDO.test("a:focus { x: y }")).toBe(true);
    expect(FOCUS_PSEUDO.test("a:focus-visible { x: y }")).toBe(false);
  });

  it("has no unlayered element selector in globals.css", () => {
    expect(unlayeredElementSelectors(readFileSync(join(SRC, "app/globals.css"), "utf8"))).toEqual([]);
  });

  it("has no other global stylesheet", () => {
    expect(files.filter((f) => f.endsWith(".css")).map(rel)).toEqual(["app/globals.css"]);
  });

  it("styles no `:focus` without `-visible` (no ring after a mouse click)", () => {
    expect(offenders(FOCUS_UTILITY)).toEqual([]);
    expect(offenders(FOCUS_PSEUDO, new Set(["components/agents/files/viewer/editor.tsx"]))).toEqual([]);
  });
});

