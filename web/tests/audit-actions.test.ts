import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { AUDIT_SENTENCE, auditActionLabel, auditActor, auditFilterGroups, auditWords } from "@/lib/labels";

// (P5-REFINE-20, workspace-3, admin-10) Every action the API writes to the audit log has a sentence: the fallback "did
// something (“…”)" is a failure here, for any action on the list the Rust call sites give.

const ROOT = join(__dirname, "..", "..");
const FIXTURE = JSON.parse(readFileSync(join(__dirname, "fixtures", "audit-actions.json"), "utf8")) as { actions: string[] };

/** Every non-test `.rs` file under `dir`. */
function rustFiles(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return name === "tests" || name === "target" ? [] : rustFiles(p);
    return name.endsWith(".rs") ? [p] : [];
  });
}

const ACTION = '"([a-z][a-z0-9_]*(?:\\.[a-z][a-z0-9_]*)+)"';
const NOT_ACTIONS = /^(aurora|http|db|otel|url|email)\./;

/** The actions the Rust call sites name: `AuditEvent::by/new(.., "x")`, `action: "x"` fields and the security-critical set. */
function actionsInRust(): string[] {
  const found = new Set<string>();
  const sources = [join(ROOT, "services", "aurora-api", "src"), ...readdirSync(join(ROOT, "crates")).map((c) => join(ROOT, "crates", c, "src"))];
  for (const file of sources.flatMap(rustFiles)) {
    const text = readFileSync(file, "utf8");
    for (const m of text.matchAll(new RegExp(`AuditEvent::(?:by|new)\\([\\s\\S]{0,400}?${ACTION}`, "g"))) found.add(m[1]!);
    for (const m of text.matchAll(new RegExp(`\\baction:\\s*${ACTION}`, "g"))) found.add(m[1]!);
  }
  const critical = readFileSync(join(ROOT, "crates", "aurora-core", "src", "audit.rs"), "utf8");
  const block = /SECURITY_CRITICAL_ACTIONS[^=]*=\s*&\[([\s\S]*?)\];/.exec(critical)?.[1] ?? "";
  for (const m of block.matchAll(new RegExp(ACTION, "g"))) found.add(m[1]!);
  return [...found].filter((a) => !NOT_ACTIONS.test(a)).sort();
}

describe("audit actions in words", () => {
  it("has a sentence template for every action the API writes, and none says 'did something'", () => {
    const missing = FIXTURE.actions.filter((a) => !(a in AUDIT_SENTENCE));
    expect(missing, "an action the API writes needs its template in lib/labels.ts (AUDIT_SENTENCE)").toEqual([]);
    for (const action of FIXTURE.actions) {
      const w = auditWords({ action, actor_label: "alice@example.test", actor_kind: "user", actor_name: "Alice Admin", metadata: {} }, null);
      expect(w.known, action).toBe(true);
      expect(w.sentence, action).not.toMatch(/did something/);
      // No dotted key, no machine slug and no e-mail address in what a person reads.
      expect(w.sentence, action).not.toMatch(/[a-z]\.[a-z]|_|@/);
      expect(w.sentence.startsWith("Alice ") || /^(A|An|The|Attachment|Aurora)\b/.test(w.sentence), `${action}: ${w.sentence}`).toBe(true);
    }
  });

  it("fails on the fallback: an action nobody wrote a template for says so", () => {
    const w = auditWords({ action: "made_up.thing_happened", actor_label: "bob@example.test", actor_name: "Bob Builder", metadata: {} }, null);
    expect(w).toMatchObject({ known: false, sentence: "Bob did something (“made up thing happened”)" });
  });

  it("lists every action the Rust call sites name, and the list is complete (drift test)", () => {
    const named = actionsInRust();
    expect(named.length, "the scan found the call sites").toBeGreaterThan(150);
    const unlisted = named.filter((a) => !FIXTURE.actions.includes(a));
    expect(unlisted, "add the action to tests/fixtures/audit-actions.json and its template to lib/labels.ts").toEqual([]);
  });

  it("names the actor: people by display name, a key, an agent and the platform's own jobs in words", () => {
    expect(auditActor({ actor_kind: "user", actor_label: "alice@example.test", actor_name: "Alice Admin" })).toEqual({ name: "Alice", full: "Alice Admin" });
    // No display name yet: the address's first part, never the address.
    expect(auditActor({ actor_kind: "user", actor_label: "alice@example.test", actor_name: null })).toEqual({ name: "Alice", full: "Alice" });
    expect(auditActor({ actor_kind: "instance_admin", actor_label: "root@example.test", actor_name: "Root Admin" }).name).toBe("Root");
    expect(auditActor({ actor_kind: "service", actor_label: "bob@example.test", actor_name: "Bob Builder" })).toEqual({ name: "Bob", full: "Bob Builder (through an API key)" });
    expect(auditActor({ actor_kind: "service", actor_label: "bob@example.test", actor_name: null })).toEqual({ name: "An API key", full: "An API key" });
    // A machine actor in words: "Aurora", with the job named underneath; a slug never shows.
    expect(auditActor({ actor_kind: "service", actor_label: "scim:okta" })).toEqual({ name: "Directory sync", full: "Directory sync" });
    expect(auditActor({ actor_kind: "service", actor_label: "idp:corp" }).name).toBe("The sign-in provider");
    expect(auditActor({ actor_kind: "system", actor_label: "placement" })).toEqual({ name: "Aurora", full: "New-user placement" });
    expect(auditActor({ actor_kind: "system", actor_label: "connector:0194-abc" })).toEqual({ name: "Aurora", full: "A chat platform" });
    expect(auditActor({ actor_kind: "system", actor_label: "some-new-job" })).toEqual({ name: "Aurora", full: "Aurora" });
    expect(auditWords({ action: "grant.create", actor_kind: "system", actor_label: "placement", critical: true, metadata: {} }, "Grumpy").sentence).toBe(
      "Aurora gave access to the account Grumpy",
    );
  });

  it("offers one 'All actions' filter in words, never a dotted key", () => {
    for (const level of ["workspace", "organization", "instance"] as const) {
      const groups = auditFilterGroups(level);
      expect(groups.length).toBeGreaterThan(2);
      for (const g of groups) {
        expect(g.group).not.toMatch(/[._]/);
        for (const o of g.options) {
          expect(o.label, o.value).not.toMatch(/[a-z]\.[a-z]|_/);
          expect(o.label).toBe(auditActionLabel(o.value));
        }
      }
    }
    // Two different actions never read the same in the list.
    for (const level of ["workspace", "instance"] as const) {
      const labels = auditFilterGroups(level).flatMap((g) => g.options.map((o) => o.label));
      expect(labels.filter((l, i) => labels.indexOf(l) !== i), `${level}: labels that repeat`).toEqual([]);
    }
    // The workspace's list leaves out what only an instance administrator does.
    const workspace = auditFilterGroups("workspace").flatMap((g) => g.options.map((o) => o.value));
    expect(workspace).toContain("api_key.revoke");
    expect(workspace).not.toContain("instance_settings.update");
    expect(auditFilterGroups("instance").flatMap((g) => g.options.map((o) => o.value))).toContain("instance_settings.update");
    expect(auditActionLabel("api_key.revoke")).toBe("Revoked an API key");
    expect(auditActionLabel("memory.save")).toBe("Saved a memory");
  });
});
