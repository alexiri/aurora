import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { AUDIT_ACTION, AUDIT_SENTENCE, auditWords, COVERED, GATEWAY_DECISION, HARNESS, harnessVisible, label, SANDBOX_PHASE } from "@/lib/labels";

const SCHEMA = readFileSync(join(__dirname, "..", "src", "lib", "api", "schema.d.ts"), "utf8");

/** The values of a string enum of `schema.d.ts` (`        Name: "a" | "b";`). */
function enumValues(name: string): string[] {
  const m = new RegExp(`^ {8}${name}: ((?:"[^"]+"(?: \\| )?)+);$`, "m").exec(SCHEMA);
  if (!m) throw new Error(`enum ${name} not found in schema.d.ts`);
  return [...m[1]!.matchAll(/"([^"]+)"/g)].map((x) => x[1]!);
}

/** The values of `values` that `table` has no label for. */
const unlabelled = (table: Record<string, unknown>, values: string[]) => values.filter((v) => !(v in table));

describe("labels (rec 13)", () => {
  it("has a label for every value of every schema enum it covers", () => {
    const missing = Object.entries(COVERED).flatMap(([name, table]) => unlabelled(table, enumValues(name)).map((v) => `${name}.${v}`));
    expect(missing, "a new enum value in the API needs its words in lib/labels.ts").toEqual([]);
  });

  it("fails when a made-up value is added to a covered enum (shown once)", () => {
    const withMadeUp = [...enumValues("HealthState"), "made_up_state"];
    expect(unlabelled(COVERED.HealthState!, withMadeUp)).toEqual(["made_up_state"]);
  });

  it("renders ? for an unknown key, never the key", () => {
    expect(label(GATEWAY_DECISION, "made_up")).toBe("?");
    expect(label(GATEWAY_DECISION, null)).toBe("?");
    expect(label(GATEWAY_DECISION, "default_denied")).toBe("Not allowed");
  });

  it("names phases, decisions, harnesses and audit actions in words", () => {
    expect(label(SANDBOX_PHASE, "running")).toBe("On");
    expect(label(SANDBOX_PHASE, "failed")).toBe("Failed");
    expect(label(HARNESS, "claude_code")).toBe("Claude Code");
    expect(label(AUDIT_ACTION, "api_key.revoke")).toBe("Revoked an API key");
    expect(GATEWAY_DECISION.default_denied.sentence()).toMatch(/no rule/);
  });

  it("has an audit sentence for every named audit action, and says so plainly when one is missing", () => {
    expect(Object.keys(AUDIT_ACTION).filter((a) => !(a in AUDIT_SENTENCE))).toEqual([]);
    const gone = { actor_label: "Bob", critical: true, metadata: {} };
    expect(auditWords({ ...gone, action: "api_key.revoke" }, "ci-script")).toMatchObject({ sentence: "Bob revoked the API key ci-script", weight: "critical" });
    expect(auditWords({ ...gone, action: "api_key.revoke" }, null).sentence).toBe("Bob revoked a deleted API key");
    expect(auditWords({ ...gone, critical: false, action: "workspace_access.set", metadata: { role: "owner" } }, "Carol")).toMatchObject({
      sentence: "Bob changed Carol to Owner",
      weight: "important",
    });
    expect(auditWords({ ...gone, critical: false, action: "workspace.access.grant", metadata: { role: "member" } }, "Carol").weight).toBe("routine");
    const unknown = auditWords({ ...gone, critical: false, action: "thing.happened" }, null);
    expect(unknown).toMatchObject({ known: false, sentence: "Bob did something (“thing happened”)" });
  });

  it("hides the fake harness outside development builds", () => {
    expect(harnessVisible("claude_code")).toBe(true);
    expect(harnessVisible("fake")).toBe(process.env.NODE_ENV === "development");
  });

  it("has no label that is its own key or contains an underscore", () => {
    for (const table of [GATEWAY_DECISION, SANDBOX_PHASE, AUDIT_ACTION, HARNESS, ...Object.values(COVERED)]) {
      for (const [key, entry] of Object.entries(table)) {
        const text = (entry as { label: string }).label;
        // A brand written as its key ("goose") is fine; a key with a machine shape never is its own label.
        if (/[_.]/.test(key)) expect(text, key).not.toBe(key);
        expect(text, key).not.toMatch(/_/);
      }
    }
  });
});
