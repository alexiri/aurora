/**
 * UI text rules (C-974, D-341): the rendered strings of `web/src/components` and `web/src/app` never cite a spec
 * section or a repository path, never show a raw enumeration id or an empty `[]` as a value, never derive a label from
 * a key, and do not use the retired words of the vocabulary table (`14 §Vocabulary`).
 *
 * It reads strings, not comments or identifiers: the source is parsed and only string literals, template text, JSX
 * text and the text attributes of JSX elements are looked at.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

import ts from "typescript";
import { describe, expect, it } from "vitest";

const WEB = join(__dirname, "..");
const ROOTS = ["src/components", "src/app"];

interface Hit {
  file: string;
  line: number;
  text: string;
  /** `jsx` rendered text, `attr` a text attribute, `string` any other string literal. */
  where: "jsx" | "attr" | "string";
}

/** JSX attributes whose value is shown or read out. */
const TEXT_ATTRS = new Set(["aria-label", "title", "placeholder", "alt", "label", "legend", "description", "confirmLabel", "submitLabel", "emptyLabel", "aria-description"]);

function files(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...files(p));
    else if (/\.(ts|tsx)$/.test(name) && !name.endsWith(".d.ts") && !/\.test\./.test(name)) out.push(p);
  }
  return out;
}

function stringsOf(path: string): Hit[] {
  const text = readFileSync(path, "utf8");
  const sf = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, path.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const file = relative(WEB, path);
  const hits: Hit[] = [];
  const add = (node: ts.Node, value: string, where: Hit["where"]) => {
    const t = value.replace(/\s+/g, " ").trim();
    if (!t) return;
    hits.push({ file, line: sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1, text: t, where });
  };
  const visit = (node: ts.Node) => {
    const parent = node.parent;
    if (ts.isJsxText(node)) add(node, node.text, "jsx");
    else if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      const skip =
        ts.isImportDeclaration(parent) ||
        ts.isExportDeclaration(parent) ||
        ts.isLiteralTypeNode(parent) ||
        (ts.isPropertyAssignment(parent) && parent.name === node) ||
        (ts.isElementAccessExpression(parent) && parent.argumentExpression === node) ||
        (ts.isCallExpression(parent) && ts.isIdentifier(parent.expression) && parent.expression.text === "require");
      if (!skip) {
        if (ts.isJsxAttribute(parent)) {
          if (TEXT_ATTRS.has(parent.name.getText(sf))) add(node, node.text, "attr");
        } else add(node, node.text, "string");
      }
    } else if (ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node)) add(node, node.text, "string");
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return hits;
}

const ALL: Hit[] = ROOTS.flatMap((r) => files(join(WEB, r))).flatMap(stringsOf);

const fmt = (hs: Hit[]) => hs.map((h) => `${h.file}:${h.line}  ${JSON.stringify(h.text.slice(0, 90))}`);

const RAW_IDS = /\b(sanctioned_only|credentialed_only|owners_only|allowlist_only|open_lane)\b/;

describe("UI text rules (C-974)", () => {
  /** Strings of the files a brief of wave 2 or 3 rewrites (`OTHER_EXEMPT`, below) are checked when that brief lands. */
  const checked = (h: Hit) => !(h.file in OTHER_EXEMPT);

  it("never cites a spec section or a repository path", () => {
    const bad = ALL.filter(checked).filter((h) => /§\s?\d/.test(h.text) || h.text.includes("docs/"));
    expect(fmt(bad)).toEqual([]);
  });

  it("never shows a raw enumeration id, or an empty list as `[]`, as text", () => {
    // An id on its own is a value in code (a comparison, an option's value); in a sentence, a label or JSX text it is
    // shown. A string that starts with `/` is a path into a document (an error's location), not text.
    const bad = ALL.filter(checked).filter(
      (h) =>
        (RAW_IDS.test(h.text) && !h.text.startsWith("/") && (h.where !== "string" || h.text !== h.text.match(RAW_IDS)?.[0])) ||
        (h.where !== "string" ? h.text.includes("[]") : h.text === "[]"),
    );
    expect(fmt(bad)).toEqual([]);
  });

  it("has no function that derives a label from a key", () => {
    const text = ROOTS.flatMap((r) => files(join(WEB, r)))
      .map((f) => ({ f: relative(WEB, f), s: readFileSync(f, "utf8") }))
      .filter(({ s }) => /\b(labelOf|labelFromKey|keyToLabel|humanizeKey|humanizeSettingKey)\b/.test(s) || /replace\(\/_\/g, " "\)\s*;?\s*\n?\s*return\s+\w+\.charAt\(0\)\.toUpperCase/.test(s))
      .map(({ f }) => f);
    expect(text).toEqual([]);
  });

  // ---------------------------------------------------------------- vocabulary (14 §Vocabulary)
  /** Retired words used as a page name, a label or a button: capitalised, whole word. */
  const RETIRED =
    /\b(Governance|Policy|Policies|Credentials|Credential|Simulate|Simulation|Disable|Enable|Freeze|Unfreeze|Frozen|Triggers|Evals|Peers|Tighten|Tightened|Redact|Redacting)\b|Rules people wrote|Disabled \(user\)|Gate command|Provider endpoint|Limits from above|Default agent|Check what would happen\b(?!:)|What moved/;
  /** (D-407) "OAuth client" is only a tab id and code; where a person reads it, it is "Sign-in app". */
  const OAUTH_CLIENT = /OAuth clients?/;
  /** A string that is only a retired noun for an account (`grant`, `secret`, `connection`), used as a label. */
  const RETIRED_LABEL = /^(Grants?|Secrets?|Connections?)$/;
  const retired = (h: Hit) => RETIRED.test(h.text) || RETIRED_LABEL.test(h.text);

  /**
   * Files that still use a retired word and that a brief of wave 2 or 3 rewrites (C-974). The list can only shrink: the
   * test fails when a listed file no longer has a hit, so the brief that cleans a file removes its entry. It must be
   * empty when wave 3 is done.
   */
  const VOCABULARY_EXEMPT: Record<string, string> = {};

  /**
   * Files a brief of wave 2 or 3 rewrites that also still hold a spec reference or a raw id (they leave with the same
   * list entry: the brief that rewrites the file removes it).
   */
  const OTHER_EXEMPT: Record<string, string> = {};

  it("does not use a retired word of the vocabulary table as a name, label or button", () => {
    const byFile = new Map<string, Hit[]>();
    for (const h of ALL.filter(retired)) byFile.set(h.file, [...(byFile.get(h.file) ?? []), h]);
    const unlisted = [...byFile.entries()].filter(([f]) => !(f in VOCABULARY_EXEMPT)).flatMap(([, hs]) => fmt(hs));
    expect(unlisted).toEqual([]);
    const stale = Object.keys(VOCABULARY_EXEMPT).filter((f) => !byFile.has(f));
    expect(stale, "listed files that no longer contain a retired word: remove them").toEqual([]);
  });

  it("says Sign-in app, never OAuth client, where a person reads it (D-407)", () => {
    // The only strings that may hold the words are a tab id (`oauth-clients`, `oauth_client`) and a docs-style note in a sentence about code.
    const bad = ALL.filter((h) => OAUTH_CLIENT.test(h.text));
    expect(fmt(bad)).toEqual([]);
  });

  it("never names a person where an admin is meant (D-407)", () => {
    // A literal "Ask alice", a "managed by" followed by a template expression of names, or the connector admins' names.
    const bad = ALL.filter((h) => /\bAsk (alice|bob|ada|alan)\b/i.test(h.text) || /managed by \$\{/i.test(h.text) || /\bAsk: \$\{/.test(h.text));
    expect(fmt(bad)).toEqual([]);
  });

  it("lists a file in the other exemptions only while it still has a spec reference or a raw id", () => {
    const hasOther = (f: string) => ALL.some((h) => h.file === f && (/§\s?\d/.test(h.text) || h.text.includes("docs/") || RAW_IDS.test(h.text)));
    expect(Object.keys(OTHER_EXEMPT).filter((f) => !hasOther(f))).toEqual([]);
  });
});
