import { readFileSync } from "node:fs";
import { join } from "node:path";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

// FIX-276: `cn()` must know the theme's own font sizes and tracking. With tailwind-merge's default config `text-label`
// reads as a text colour, so a real colour next to it dropped the size (badges rendered at 14 px, not 11 px).

afterEach(cleanup);

/** The names of the theme's own `--<ns>-<name>` variables in globals.css. */
function themeNames(ns: "text" | "tracking"): string[] {
  const css = readFileSync(join(process.cwd(), "src/app/globals.css"), "utf8");
  return [...css.matchAll(new RegExp(`^\\s*--${ns}-([a-z0-9-]+):`, "gm"))].map((m) => m[1]);
}

const classes = (s: string) => s.split(" ");

describe("cn() and the theme's own sizes", () => {
  it("globals.css still declares them", () => {
    expect(themeNames("text")).toEqual(expect.arrayContaining(["label", "micro"]));
    expect(themeNames("tracking")).toEqual(expect.arrayContaining(["label", "brand"]));
  });

  for (const name of themeNames("text")) {
    it(`text-${name} is a size: it stays next to a colour, either way round`, () => {
      expect(cn(`text-${name}`, "text-ink-2")).toBe(`text-${name} text-ink-2`);
      expect(cn("text-pending-ink", `text-${name}`)).toBe(`text-pending-ink text-${name}`);
    });

    it(`text-${name} conflicts with the other sizes: the last one wins`, () => {
      expect(cn(`text-${name}`, "text-sm")).toBe("text-sm");
      expect(cn("text-sm", `text-${name}`)).toBe(`text-${name}`);
    });
  }

  it("the theme's two sizes conflict with each other", () => {
    expect(cn("text-label", "text-micro")).toBe("text-micro");
  });

  for (const name of themeNames("tracking")) {
    it(`tracking-${name} conflicts with the other tracking utilities: the last one wins`, () => {
      expect(cn(`tracking-${name}`, "tracking-wide")).toBe("tracking-wide");
      expect(cn("tracking-wide", `tracking-${name}`)).toBe(`tracking-${name}`);
    });
  }
});

describe("Badge keeps its label size", () => {
  for (const tone of ["neutral", "accent", "danger"] as const) {
    it(`${tone} tone`, () => {
      render(<Badge tone={tone}>Text</Badge>);
      expect(classes(screen.getByText("Text").className)).toContain("text-label");
    });
  }

  it("with a caller's colour", () => {
    render(<Badge className="text-muted">Text</Badge>);
    expect(classes(screen.getByText("Text").className)).toEqual(expect.arrayContaining(["text-label", "text-muted"]));
  });
});
