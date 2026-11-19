import { render, screen } from "@testing-library/react";
import Link from "next/link";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { Button, buttonVariants } from "@/components/ui/button";

// (P5-POLISH-1, D-393) A link styled as a button reads its colours from the same tokens as the button. jsdom does not
// run Tailwind, so the contrast is computed from the token values in globals.css for each theme; the layer guard in
// design-guards.test.ts keeps an unlayered `a { color }` from overriding these utilities.

const CSS = readFileSync(join(__dirname, "..", "src/app/globals.css"), "utf8");

function tokens(block: RegExp): Record<string, string> {
  const m = CSS.match(block);
  const out: Record<string, string> = {};
  for (const [, k, v] of (m?.[1] ?? "").matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]{6})\s*;/g)) out[k] = v;
  return out;
}
const LIGHT = tokens(/\n:root \{([\s\S]*?)\n\}/);
const DARK = { ...LIGHT, ...tokens(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/) };

function luminance(hex: string): number {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0]! + 0.7152 * c[1]! + 0.0722 * c[2]!;
}
function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

const TEXT_SIZE = new Set(["sm", "base", "lg", "xs", "label", "micro"]);
function colours(cls: string, theme: Record<string, string>) {
  const names = cls.split(/\s+/).filter((c) => !c.includes(":"));
  const text = names.find((c) => c.startsWith("text-") && !TEXT_SIZE.has(c.slice(5)))?.slice(5);
  const bg = names.find((c) => c.startsWith("bg-"))?.slice(3) ?? "canvas";
  return { fg: theme[text ?? "ink"]!, bg: theme[bg]!, text, bgName: bg };
}

const VARIANTS = ["primary", "outline", "approve", "ghost", "danger"] as const;

describe("a link styled as a button", () => {
  for (const [name, theme] of [["light", LIGHT], ["dark", DARK]] as const) {
    for (const variant of VARIANTS) {
      it(`${variant} has readable text in the ${name} theme (>= 4.5:1)`, () => {
        const cls = buttonVariants({ variant });
        const { fg, bg, text, bgName } = colours(cls, theme);
        expect(fg, `token for text-${text}`).toBeTruthy();
        expect(bg, `token for bg-${bgName}`).toBeTruthy();
        expect(fg).not.toBe(bg);
        expect(contrast(fg, bg)).toBeGreaterThanOrEqual(4.5);
      });
    }
  }

  it("renders the variant utilities on an <a> through asChild and through buttonVariants on a Link", () => {
    render(
      <>
        <Button asChild>
          <a href="/a">as child</a>
        </Button>
        <Link href="/b" className={buttonVariants({ variant: "primary" })}>
          on a link
        </Link>
      </>,
    );
    for (const name of ["as child", "on a link"]) {
      const a = screen.getByRole("link", { name });
      expect(a.tagName).toBe("A");
      expect(a.className).toContain("text-on-primary");
      expect(a.className).toContain("bg-primary");
    }
  });
});
