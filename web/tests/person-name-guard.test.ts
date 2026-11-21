/**
 * The person-name guard (P5-REFINE-20, batch 9 item 3): a person's name is never shown raw. A person stored by their
 * address has that address as `display_name`, so every place that shows one goes through `personName(...)` (or
 * `firstNameOf(...)` for a first name), which turns an address into a name.
 *
 * The test reads `src/` and fails on a line that holds `display_name` and neither calls one of the two helpers, nor
 * reads a thing that is not a person (a connector, an AI endpoint, a sign-in provider, a harness, a model: `NOT_A_PERSON`),
 * nor sits in `ALLOWED` below: the few places that use the field on purpose, each with its reason.
 */
import { readFileSync } from "node:fs";
import { join, relative } from "node:path";

import { describe, expect, it } from "vitest";

import { files, WEB } from "./support/ui-strings";

/** A line that calls a helper which makes a name out of an address. */
const WRAPPED = /\b(personName|firstNameOf)\(/;
/** Receivers whose `display_name` is a product's, not a person's (connectors, endpoints, providers, harnesses, models). */
const NOT_A_PERSON = /\b(connector|endpoint|harness|provider|shared_bot|existing|myProvider|[cdehmpv])\??\.display_name\b|\bdisplay_name\??:\s*(string|null)|text\(\w+\.display_name\)/;
/** A type declaration, a form field of a non-person, an API payload key. */
const DECLARATION = /\bdisplay_name\??:|["']display_name["']|fe\.display_name|errors\.display_name|d\.display_name|set\("display_name"|put\("display_name"/;

/** File → text the line holds → why showing the field as it is stays. */
const ALLOWED: { file: string; has: string; why: string }[] = [
  { file: "src/components/account/profile-view.tsx", has: "me.user.display_name", why: "the person's own name in the box where they edit it: the stored text, not a shown name" },
  { file: "src/components/account/admin-users-view.tsx", has: "u.display_name.includes", why: "decides whether to show the address or its local part under the name; the name itself goes through personName" },
  { file: "src/components/admin/settings/settings-section-form.tsx", has: "e.display_name", why: "an AI endpoint's name" },
  { file: "src/components/admin/settings/llm-endpoint-dialog.tsx", has: "display_name", why: "an AI endpoint's name" },
  { file: "src/components/admin/settings/sign-in-provider-dialog.tsx", has: "display_name", why: "a sign-in provider's name" },
  { file: "src/components/admin/settings/sign-in-settings-view.tsx", has: "display_name", why: "a sign-in provider's or endpoint's name" },
  { file: "src/components/admin/settings/provider-roles-view.tsx", has: "display_name", why: "a sign-in provider's name" },
  { file: "src/components/agents/model-roles-fields.tsx", has: "display_name", why: "a model's name" },
  { file: "src/components/agents/instructions-tab.tsx", has: "display_name", why: "a model's or harness's name" },
  { file: "src/components/auth/login-view.tsx", has: "display_name", why: "a sign-in provider's name" },
  { file: "src/components/groups/create-group-dialog.tsx", has: "display_name", why: "a sign-in provider's name" },
  { file: "src/components/channels/connected-done.tsx", has: "display_name", why: "a chat platform's name" },
  { file: "src/components/automations/automations-tab.tsx", has: "display_name", why: "a chat platform's name" },
  { file: "src/components/connectors/", has: "display_name", why: "a chat platform's or connector's name" },
  { file: "src/components/credentials/llm-form.tsx", has: "ep.display_name", why: "an AI endpoint's name" },
  { file: "src/lib/api/", has: "display_name", why: "API types and request payloads" },
];

function lineAllowed(file: string, line: string): boolean {
  return ALLOWED.some((a) => file.startsWith(a.file) && line.includes(a.has));
}

describe("person names are shown through personName", () => {
  it("no component, route or helper renders a person's display_name raw", () => {
    const offenders: string[] = [];
    for (const f of files(join(WEB, "src"))) {
      const rel = relative(WEB, f).replaceAll("\\", "/");
      if (rel.endsWith("schema.d.ts")) continue;
      readFileSync(f, "utf8")
        .split("\n")
        .forEach((line, i) => {
          if (!/(?<![A-Za-z_])display_name/.test(line)) return;
          if (line.trimStart().startsWith("//") || line.trimStart().startsWith("*")) return;
          if (WRAPPED.test(line) || NOT_A_PERSON.test(line) || DECLARATION.test(line) || lineAllowed(rel, line)) return;
          offenders.push(`${rel}:${i + 1}: ${line.trim()}`);
        });
    }
    expect(offenders, `show a person's name with personName(...) from "@/lib/workspace-name":\n${offenders.join("\n")}`).toEqual([]);
  });

  it("the guard sees a raw name (self-check)", () => {
    const raw = "<p>{me.data.user.display_name}</p>";
    expect(raw.includes("display_name") && !WRAPPED.test(raw) && !NOT_A_PERSON.test(raw) && !DECLARATION.test(raw)).toBe(true);
    const wrapped = "<p>{personName(me.data.user.display_name)}</p>";
    expect(WRAPPED.test(wrapped)).toBe(true);
  });
});
