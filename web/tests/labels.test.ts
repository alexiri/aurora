import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { AUDIT_ACTION, AUDIT_SENTENCE, auditWords, COVERED, GATEWAY_DECISION, HARNESS, harnessVisible, label, LIBRARY_STATUS, PEER_TASK_STATUS, SANDBOX_PHASE, SKILL_INVALID_REASON, skillInvalidWords } from "@/lib/labels";

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

  it("says every skill library status in words (P5-SKLIB-3, D-450(e))", () => {
    expect(["active", "paused", "error", "over_budget"].map((s) => label(LIBRARY_STATUS, s))).toEqual(["Up to date", "Paused", "Needs attention", "Over budget"]);
  });

  it("says every reason a library skill can't be added in a sentence, and a plain one for a code it does not know (FIX-279)", () => {
    const codes = ["too_large", "body_too_large", "file_too_large", "too_many_files", "unsupported_path", "unsupported_entry", "skill_md", "bad_name", "unknown_size"];
    expect(Object.keys(SKILL_INVALID_REASON).sort()).toEqual([...codes].sort());
    const sentences = codes.map((c) => skillInvalidWords(c));
    for (const text of sentences) expect(text).toMatch(/^\p{Lu}.*\.$/u);
    expect(new Set(sentences).size).toBe(codes.length);
    expect(skillInvalidWords("too_large")).toBe("This skill is larger than 32,000 characters, the most a skill may hold.");
    expect(skillInvalidWords("made_up")).toBe("This skill can’t be added.");
    expect(skillInvalidWords(null)).toBe("This skill can’t be added.");
    expect(skillInvalidWords("constructor")).toBe("This skill can’t be added.");
  });

  it("says a waiting peer task in words (FIX-253, D-490(b))", () => {
    expect(label(PEER_TASK_STATUS, "waiting")).toBe("waiting for its wake-up");
    expect(label(PEER_TASK_STATUS, "open")).toBe("in progress");
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
    for (const table of [GATEWAY_DECISION, SANDBOX_PHASE, AUDIT_ACTION, HARNESS, LIBRARY_STATUS, ...Object.values(COVERED)]) {
      for (const [key, entry] of Object.entries(table)) {
        const text = (entry as { label: string }).label;
        // A brand written as its key ("goose") is fine; a key with a machine shape never is its own label.
        if (/[_.]/.test(key)) expect(text, key).not.toBe(key);
        expect(text, key).not.toMatch(/_/);
      }
    }
  });
});
