import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

import { describe, expect, it } from "vitest";

// FIX-173 (D-375): every secret field is a `SecretInput`, so Show / Hide, the no-spellcheck attributes and the
// hide-again rules cannot be forgotten by a new field.

const SRC = join(__dirname, "..", "src");
const PRIMITIVE = ["components", "ui", "secret-input.tsx"].join(sep);

function tsxFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return tsxFiles(p);
    return p.endsWith(".tsx") ? [p] : [];
  });
}

/** `type="password"`, `type={"password"}` or a `"password"` branch inside a `type=` expression. */
export function passwordInputs(source: string): string[] {
  const out: string[] = [];
  const re = /\btype\s*=\s*(?:"password"|\{[^}]*["'`]password["'`][^}]*\})/g;
  for (const m of source.matchAll(re)) out.push(m[0]);
  return out;
}

describe("secret fields use SecretInput", () => {
  it("finds a planted password input (the guard works)", () => {
    expect(passwordInputs('<input type="password" />')).toHaveLength(1);
    expect(passwordInputs("<input type={'password'} />")).toHaveLength(1);
    expect(passwordInputs('<input type={show ? "text" : "password"} />')).toHaveLength(1);
    expect(passwordInputs('<input type="text" />')).toHaveLength(0);
  });

  it("has no password input outside components/ui/secret-input.tsx — use SecretInput", () => {
    const offenders = tsxFiles(SRC)
      .filter((f) => relative(SRC, f) !== PRIMITIVE)
      .filter((f) => passwordInputs(readFileSync(f, "utf8")).length > 0)
      .map((f) => relative(SRC, f));
    expect(offenders, "a secret field is written with SecretInput (components/ui/secret-input.tsx), not type=password").toEqual([]);
  });
});
