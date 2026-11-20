/**
 * The copy guard (P5-REFINE-2, rec 13): no string a person can read is a machine word. It fails on a string that
 *
 *  (a) holds a snake_case word (`default_denied`),
 *  (b) holds a UUID, or
 *  (c) is a dotted key on its own (`governance.egress.baseline_allowlist`, `api_key.revoke`),
 *
 * unless it sits inside `<code>`, `<pre>`, `<kbd>` or `<TechnicalDetails>`. (Routes, imports, `data-testid`s and code
 * identifiers are not strings the parser looks at, and a bare string in code that is only the value itself, such as a
 * comparison or an option's value, is skipped, as in `ui-text.test.ts`.)
 *
 * `COPY_EXEMPT` lists, file by file, how many such strings a file still has. It can only shrink: the test fails when a
 * file has more than listed, and when a listed file has fewer (lower the number or remove the entry). It is the work
 * queue for the area briefs and must be empty at the end of P5-REFINE-20.
 */
import { describe, expect, it } from "vitest";

import { allStrings, type Hit } from "./support/ui-strings";

const SNAKE = /(^|[^A-Za-z0-9_.\-/])[a-z]+(_[a-z0-9]+)+(?![A-Za-z0-9_])/;
const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i;
const DOTTED = /^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$/;
const FILE_OR_HOST = /\.(com|org|net|io|test|md|json|ya?ml|toml|txt|sh|py|rs|js|ts|tsx|css|html?|csv|log|ini|cfg|conf|lock|png|svg|pem)$/;

/** A string that is only a value in code (an id, a comparison, an option's value): one token with no space. */
const bareValue = (h: Hit) => h.where === "string" && !/\s/.test(h.text);

/**
 * Names that are really called that, because the person types or reads them elsewhere (a key file, a file of the ssh
 * client, a token scope of GitHub/GitLab, the example of an import file), and a source file that holds code, not words.
 */
const REAL_NAMES = /\b(id_ed25519|known_hosts|read_repository|aurora_settings)\b/;
const CODE_FILES = new Set(["src/components/theme/theme.ts"]);

export function machineWord(h: Hit): boolean {
  if (h.technical || bareValue(h) || CODE_FILES.has(h.file) || REAL_NAMES.test(h.text)) return false;
  if (h.text.startsWith("/") || h.text.startsWith("http")) return false;
  if (UUID.test(h.text)) return true;
  if (SNAKE.test(h.text)) return true;
  return DOTTED.test(h.text) && !FILE_OR_HOST.test(h.text) && h.text.split(".").length >= 3;
}

/** file → how many machine words it still shows. Only ever smaller. */
const COPY_EXEMPT: Record<string, number> = {};

describe("copy guard (rec 13)", () => {
  const byFile = new Map<string, Hit[]>();
  for (const h of allStrings().filter(machineWord)) byFile.set(h.file, [...(byFile.get(h.file) ?? []), h]);
  const fmt = (hs: Hit[]) => hs.map((h) => `${h.file}:${h.line}  ${JSON.stringify(h.text.slice(0, 90))}`);

  it("shows no snake_case word, UUID or dotted key outside Technical details", () => {
    const over = [...byFile.entries()].filter(([f, hs]) => hs.length > (COPY_EXEMPT[f] ?? 0)).flatMap(([, hs]) => fmt(hs));
    expect(over).toEqual([]);
  });

  it("keeps the exemption list honest: a listed file must still have that many", () => {
    const stale = Object.entries(COPY_EXEMPT).filter(([f, n]) => (byFile.get(f)?.length ?? 0) < n);
    expect(stale, "lower the number or remove the entry").toEqual([]);
  });

  it("recognises machine words (the rule itself)", () => {
    const h = (text: string, where: Hit["where"] = "jsx", technical = false): Hit => ({ file: "x", line: 1, text, where, technical });
    expect(machineWord(h("Blocked: default_denied"))).toBe(true);
    expect(machineWord(h("Request 3f2c8a10-1b2c-4d3e-8f90-0123456789ab failed"))).toBe(true);
    expect(machineWord(h("governance.egress.baseline_allowlist"))).toBe(true);
    expect(machineWord(h("api_key.revoke"))).toBe(true);
    expect(machineWord(h("default_denied", "string"))).toBe(false); // a value in code
    expect(machineWord(h("default_denied", "jsx", true))).toBe(false); // inside <code>
    expect(machineWord(h("Allowed"))).toBe(false);
    expect(machineWord(h("api.example.com"))).toBe(false);
  });
});
