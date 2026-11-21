import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// docs/14 §Visual design (FIX-204): a form field is `Field` + `inputClass` (or `Select`, `Combobox`): label, 6 px gap,
// `border-line-strong`, hint and error linked by `aria-describedby`. A hand-rolled `<label className="block …">` that
// wraps an input, or an `<input>` / `<select>` / `<textarea>` with its own `border-line` classes instead of
// `inputClass`, is a one-off the owner kept finding (the account Rename's "Name"). A wrapped checkbox or radio is not a
// field. The allow-list starts empty; adding to it needs a reason a reviewer accepts.

const ROOTS = ["src/components", "src/app"];

/** file (relative to `web/`) → the offending tag's start (first 60 chars), with the reason it cannot be a `Field`. */
const ALLOWED: Record<string, string[]> = {};

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? files(p) : /\.tsx$/.test(name) ? [p] : [];
  });
}

/** The text of the JSX opening tag that starts at `from` (`<input …>`): braces and quotes are skipped, so `=>` does not end it. */
function openingTag(src: string, from: number): string {
  let depth = 0;
  let quote = "";
  for (let i = from; i < src.length; i++) {
    const c = src[i];
    if (quote) {
      if (c === quote && src[i - 1] !== "\\") quote = "";
    } else if (c === '"' || c === "'" || c === "`") {
      if (depth > 0 || c === '"') quote = c;
    } else if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (c === ">" && depth === 0) return src.slice(from, i + 1);
  }
  return src.slice(from);
}

function isChoice(tag: string): boolean {
  return /type=("|\{"?)(checkbox|radio)/.test(tag);
}

export function offences(src: string): string[] {
  const out: string[] = [];
  for (const m of src.matchAll(/<label\b/g)) {
    const tag = openingTag(src, m.index!);
    if (!/className="block\b/.test(tag)) continue;
    const end = src.indexOf("</label>", m.index!);
    const body = src.slice(m.index! + tag.length, end < 0 ? undefined : end);
    const inputs = [...body.matchAll(/<input\b/g)].map((i) => openingTag(body, i.index!));
    if (/<(select|textarea)\b/.test(body) || inputs.some((t) => !isChoice(t))) out.push(`${tag.slice(0, 60)} wraps a field`);
  }
  for (const m of src.matchAll(/<(input|select|textarea)\b/g)) {
    const tag = openingTag(src, m.index!);
    if (isChoice(tag)) continue;
    if (/\bborder-line(?![-\w])/.test(tag) && !/inputClass/.test(tag)) out.push(`${tag.slice(0, 60)} has border-line without inputClass`);
  }
  return out;
}

describe("no hand-rolled form fields", () => {
  it("every field in src/components and src/app is Field + inputClass / Select", () => {
    const found: string[] = [];
    for (const root of ROOTS) {
      for (const f of files(root)) {
        const allowed = ALLOWED[f] ?? [];
        for (const o of offences(readFileSync(f, "utf8"))) {
          if (!allowed.some((a) => o.startsWith(a))) found.push(`${f}: ${o}`);
        }
      }
    }
    expect(found).toEqual([]);
  });

  it("fails on a planted hand-rolled field", () => {
    expect(offences(`<label className="block space-y-1 text-sm">Name<input value={v} onChange={(e) => set(e.target.value)} /></label>`)).toHaveLength(1);
    expect(offences(`<label className="block text-sm">Pick<select value={v}><option>a</option></select></label>`)).toHaveLength(1);
    expect(offences(`<input className="block h-10 w-full rounded-control border border-line bg-surface px-3" value={v} onChange={(e) => set(e.target.value)} />`)).toHaveLength(1);
  });

  it("accepts the shared primitives, a wrapped checkbox and a wrapped radio", () => {
    expect(offences(`<Field id="x" label="Name"><input id="x" className={inputClass} value={v} /></Field>`)).toEqual([]);
    expect(offences(`<label className="block"><input type="checkbox" checked={c} onChange={(e) => set(e.target.checked)} /> Remember</label>`)).toEqual([]);
    expect(offences(`<label className="block text-sm"><input type="radio" name="r" /> One</label>`)).toEqual([]);
    expect(offences(`<input className={\`\${inputClass} border-line\`} />`)).toEqual([]);
  });
});
