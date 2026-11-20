import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * (P5-REFINE-15, computer-8) Every ANSI colour the terminal draws is readable on the pane's own background, in both
 * themes: at least 4.5:1 (WCAG AA). The tokens are read from `app/globals.css`, so a change there is checked here.
 * Programs that print their own 256-colour or true colour are not changed by this.
 */
const CSS = readFileSync(join(__dirname, "..", "src", "app", "globals.css"), "utf8");

/** Every `--name: value;` of the rule blocks whose selector is `selector`, merged in file order. */
function tokens(selector: string): Record<string, string> {
  const out: Record<string, string> = {};
  const re = new RegExp(`^${selector.replace(/[[\]"=]/g, "\\$&")}\\s*\\{([^}]*)\\}`, "gm");
  for (const m of CSS.matchAll(re)) {
    for (const d of m[1]!.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out[d[1]!] = d[2]!.trim();
  }
  return out;
}

const light = tokens(":root");
const dark = { ...light, ...tokens(':root[data-theme="dark"]') };

const hex = (v: string, theme: Record<string, string>): string => {
  const ref = /^var\((--[\w-]+)\)$/.exec(v);
  return ref ? hex(theme[ref[1]!]!, theme) : v;
};

function luminance(color: string): number {
  const n = parseInt(color.slice(1), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

describe.each([
  ["light", light],
  ["dark", dark],
] as const)("terminal palette, %s theme", (_name, theme) => {
  const bg = hex(theme["--term-pane-bg"]!, theme);
  it.each(Array.from({ length: 16 }, (_, i) => i))("--aurora-term-color-%i is at least 4.5:1 on the pane", (i) => {
    const fg = hex(theme[`--aurora-term-color-${i}`]!, theme);
    expect(fg).toMatch(/^#[0-9a-f]{6}$/i);
    expect(contrast(fg, bg), `${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5);
  });
});
