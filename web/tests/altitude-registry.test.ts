import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

import ts from "typescript";
import { describe, expect, it } from "vitest";

import { ALTITUDE_REGISTRY, isAltitudeId } from "@/lib/altitude-registry";

/**
 * The altitude lint (P5-REFINE-3, D-406): the classification table is data (`lib/altitude-registry.ts`); every
 * `<Altitude id level>` in `src` names a registry row at that row's level, and every row that is not marked `later`
 * (a later brief creates the control) or `mark` (a rule with no wrapper: rail topics, fields inside one wrapped group)
 * has a use in the code.
 */
const WEB = join(__dirname, "..");
const SRC = join(WEB, "src");

function files(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...files(p));
    else if (/\.(ts|tsx)$/.test(name) && !name.endsWith(".d.ts")) out.push(p);
  }
  return out;
}

interface Use {
  file: string;
  id: string;
  level: string | null;
}

function altitudeUses(path: string): Use[] {
  const text = readFileSync(path, "utf8");
  if (!text.includes("<Altitude")) return [];
  const sf = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const uses: Use[] = [];
  const attr = (el: ts.JsxOpeningLikeElement, name: string): string | null => {
    for (const a of el.attributes.properties) {
      if (ts.isJsxAttribute(a) && a.name.getText(sf) === name && a.initializer && ts.isStringLiteral(a.initializer)) return a.initializer.text;
    }
    return null;
  };
  const visit = (node: ts.Node) => {
    if ((ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && node.tagName.getText(sf) === "Altitude") {
      uses.push({ file: relative(WEB, path), id: attr(node, "id") ?? "(not a literal)", level: attr(node, "level") });
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return uses;
}

const ALL_FILES = files(SRC);
const USES = ALL_FILES.flatMap(altitudeUses);

describe("altitude registry lint", () => {
  it("finds the wrappers", () => {
    expect(USES.length).toBeGreaterThan(10);
  });

  it("every <Altitude> names a registry id at the registry's level", () => {
    const bad = USES.filter((u) => {
      if (!isAltitudeId(u.id)) return true;
      const row = ALTITUDE_REGISTRY[u.id] as { altitude: string; also?: string };
      return u.level !== row.altitude && u.level !== row.also;
    }).map((u) => `${u.file}: id=${u.id} level=${u.level}`);
    expect(bad).toEqual([]);
  });

  it("every row that is not marked later or mark is used in the code", () => {
    const registryFile = "altitude-registry.ts";
    const sources = ALL_FILES.filter((f) => !f.endsWith(registryFile)).map((f) => readFileSync(f, "utf8"));
    const unused = Object.entries(ALTITUDE_REGISTRY)
      .filter(([, row]) => !("later" in row) && !("mark" in row))
      .map(([id]) => id)
      .filter((id) => !sources.some((s) => s.includes(`"${id}"`)));
    expect(unused).toEqual([]);
  });

  it("rows marked later name the brief that wraps them", () => {
    for (const [id, row] of Object.entries(ALTITUDE_REGISTRY)) {
      if ("later" in row) expect(row.later, id).toMatch(/^P5-REFINE-\d+$/);
    }
  });
});
