/**
 * The strings a person can read in `web/src/components` and `web/src/app`, parsed from the source (not comments or
 * identifiers): JSX text, the text attributes of JSX elements and string literals. Shared by the copy guard.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

import ts from "typescript";

export const WEB = join(__dirname, "..", "..");
export const ROOTS = ["src/components", "src/app"];

export interface Hit {
  file: string;
  line: number;
  text: string;
  /** `jsx` rendered text, `attr` a text attribute, `string` any other string literal. */
  where: "jsx" | "attr" | "string";
  /** True inside `<code>`, `<pre>`, `<kbd>` or `<TechnicalDetails>`: machinery that is meant to be shown as it is. */
  technical: boolean;
}

/** JSX attributes whose value is shown or read out. */
const TEXT_ATTRS = new Set(["aria-label", "title", "placeholder", "alt", "label", "legend", "description", "confirmLabel", "submitLabel", "emptyLabel", "aria-description"]);
const TECHNICAL_TAGS = new Set(["code", "pre", "kbd", "TechnicalDetails"]);

export function files(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...files(p));
    else if (/\.(ts|tsx)$/.test(name) && !name.endsWith(".d.ts") && !/\.test\./.test(name)) out.push(p);
  }
  return out;
}

function insideTechnical(node: ts.Node, sf: ts.SourceFile): boolean {
  for (let n: ts.Node | undefined = node.parent; n; n = n.parent) {
    if (ts.isJsxElement(n) && TECHNICAL_TAGS.has(n.openingElement.tagName.getText(sf))) return true;
  }
  return false;
}

export function stringsOf(path: string): Hit[] {
  const text = readFileSync(path, "utf8");
  const sf = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, path.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const file = relative(WEB, path);
  const hits: Hit[] = [];
  const add = (node: ts.Node, value: string, where: Hit["where"]) => {
    const t = value.replace(/\s+/g, " ").trim();
    if (!t) return;
    hits.push({ file, line: sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1, text: t, where, technical: insideTechnical(node, sf) });
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

export const allStrings = (): Hit[] => ROOTS.flatMap((r) => files(join(WEB, r))).flatMap(stringsOf);
