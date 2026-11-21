import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * (P5-REFINE-20 batch 7 item 5, a11y) Secondary text on a person's bubble ("Technical details" on a webhook delivery) is
 * the token `--bubble-muted`, at least 4.5:1 (WCAG AA) on `--bubble` in both themes. `--muted` on the bubble was 3.09:1
 * light and 4.36:1 dark. The tokens are read from `app/globals.css`.
 */
const CSS = readFileSync(join(__dirname, "..", "src", "app", "globals.css"), "utf8");

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

function luminance(color: string): number {
  const n = parseInt(color.slice(1), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
const ratio = (a: string, b: string) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
};

describe("secondary text on the bubble", () => {
  for (const [name, theme] of [["light", light], ["dark", dark]] as const) {
    it(`--bubble-muted is at least 4.5:1 on --bubble (${name})`, () => {
      expect(theme["--bubble-muted"]).toMatch(/^#[0-9a-f]{6}$/i);
      expect(ratio(theme["--bubble-muted"]!, theme["--bubble"]!)).toBeGreaterThanOrEqual(4.5);
    });
  }

  it("the colour is a Tailwind token, and the delivery bubble's Technical details uses it", () => {
    expect(CSS).toContain("--color-bubble-muted: var(--bubble-muted);");
    const src = readFileSync(join(__dirname, "..", "src", "components", "chat", "automation-message.tsx"), "utf8");
    expect(src).toContain('summaryClassName="text-bubble-muted"');
  });
});
