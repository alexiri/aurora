import { describe, expect, it } from "vitest";

import type { AgentExport } from "@/lib/export/agent-export";
import {
  appAccessRows,
  placeholderProblem,
  TemplateError,
  templateFiles,
  templateYaml,
  type TemplateAccountRow,
  type TemplateAppEntry,
  type TemplateGrantRow,
  type TemplateHelper,
  type TemplateSkillRef,
} from "@/lib/export/agent-template";
import { writeZip } from "@/lib/export/zip";
import { parseYaml } from "@/lib/export/yaml";

// P6-TPL-5 (C-1118): the saved template is the export with `template.yaml`, a README from "What it's
// for", schedules without `time_zone`, no pending one-time schedules, library skills as references
// and helpers with their executable bit. Never a connection id, account label, secret, webhook
// address, library token or model name in any written byte.

const enc = new TextEncoder();

const exp: AgentExport = {
  format: 1,
  name: "Daneel",
  model: "claude-sonnet-4",
  may_message_first: false,
  instructions: "Triage the team's pull requests.",
  memories: [{ key: "likes-tea", updated: "2026-11-01T09:00:00Z", content: "Ada likes tea." }],
  skills: [{ name: "release-notes", useItWhen: "When asked what shipped", body: "# Notes", files: [{ path: "run.sh", content: new Uint8Array([1, 2, 3]) }] }],
  automations: [
    { kind: "schedule", name: "Morning", prompt: "Check the queue.", cadence: { cron: "0 8 * * 1-5" }, time_zone: "Europe/Zurich", enabled: true },
    { kind: "schedule", name: "Reminder", prompt: "Call Ada.", cadence: { preset: "once", run_at: "2026-12-01T09:00" }, time_zone: "Europe/Zurich", enabled: true },
    { kind: "webhook", name: "Deploy notice", instructions: "Say what shipped.", verification: "signed", enabled: true },
  ],
};

const meta = { id: "pr-triage", version: "1.0.0", name: "PR triage", summary: "Triage the pull-request queue.", readme: "## What Daneel does\n\nTriage." };
const apps: TemplateAppEntry[] = [{ app: "github", required: true, level: "act", why: "Reads and merges pull requests." }];
const refs: TemplateSkillRef[] = [{ library: "https://gitlab.example.org/agents/skills.git", skill: "incident-writeup", required: true, why: "Writes the incident summary." }];
const helpers: TemplateHelper[] = [{ path: "scripts/run.sh", content: new TextEncoder().encode("#!/bin/sh\necho hi\n"), executable: true }];
const now = new Date("2026-11-21T12:00:00Z");

const files = (e: AgentExport = exp, m = meta) => templateFiles(e, m, apps, refs, helpers, now);
const zipBytes = (e: AgentExport = exp) => writeZip(files(e), now);

/** Every file's name. */
const names = (fs: ReturnType<typeof files>) => fs.map((f) => f.name);

/** The external attributes of each central-directory record: the executable bit rides in the high bytes. */
function attrs(bytes: Uint8Array): Map<string, number> {
  const v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let eocd = -1;
  for (let i = bytes.length - 22; i >= 0; i--) {
    if (v.getUint32(i, true) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  const count = v.getUint16(eocd + 10, true);
  let pos = v.getUint32(eocd + 16, true);
  const out = new Map<string, number>();
  for (let i = 0; i < count; i++) {
    const nlen = v.getUint16(pos + 28, true);
    out.set(new TextDecoder().decode(bytes.subarray(pos + 46, pos + 46 + nlen)), v.getUint32(pos + 38, true));
    pos += 46 + nlen + v.getUint16(pos + 30, true) + v.getUint16(pos + 32, true);
  }
  return out;
}

const asText = (fs: ReturnType<typeof files>, name: string) => new TextDecoder().decode(fs.find((f) => f.name === name)!.data);

describe("templateFiles", () => {
  it("writes template.yaml first and puts every path under <id>/", () => {
    expect(names(files())).toEqual([
      "pr-triage/template.yaml",
      "pr-triage/agent.yaml",
      "pr-triage/instructions.md",
      "pr-triage/memory/likes-tea.md",
      "pr-triage/skills/release-notes/SKILL.md",
      "pr-triage/skills/release-notes/run.sh",
      "pr-triage/automations/morning.yaml",
      "pr-triage/automations/deploy-notice.yaml",
      "pr-triage/README.md",
      "pr-triage/helpers/scripts/run.sh",
    ]);
  });

  it("renames the root to the id even when the id is longer than a slug", () => {
    const id = "a-very-long-template-identifier-that-goes-beyond-the-sixty-character-slug-limit";
    const fs = templateFiles(exp, { ...meta, id }, apps, refs, helpers, now);
    expect(fs.every((f) => f.name.startsWith(`${id}/`))).toBe(true);
  });

  it("keeps the ticked groups only: an export without memories and skills writes neither", () => {
    const fs = files({ ...exp, memories: [], skills: [] });
    expect(names(fs).filter((n) => n.startsWith("pr-triage/memory/") || n.startsWith("pr-triage/skills/"))).toEqual([]);
    expect(names(files({ ...exp, automations: [] })).filter((n) => n.startsWith("pr-triage/automations/"))).toEqual([]);
  });

  it("writes agent.yaml with model: default and the schedules without time_zone", () => {
    expect(parseYaml(asText(files(), "pr-triage/agent.yaml"))).toMatchObject({ format: 1, name: "Daneel", model: "default" });
    const morning = parseYaml(asText(files(), "pr-triage/automations/morning.yaml")) as Record<string, unknown>;
    expect(morning).toMatchObject({ kind: "schedule", name: "Morning", cadence: { cron: "0 8 * * 1-5" }, enabled: true });
    expect(morning).not.toHaveProperty("time_zone");
  });

  it("leaves pending one-time schedules out", () => {
    const fs = files();
    expect(names(fs).join("\n")).not.toContain("reminder");
    expect(new TextDecoder().decode(zipBytes())).not.toContain("run_at");
  });

  it("writes a library skill as a reference and never its content", () => {
    const yaml = parseYaml(asText(files(), "pr-triage/template.yaml")) as Record<string, unknown>;
    expect(yaml.skills).toEqual([{ library: "https://gitlab.example.org/agents/skills.git", skill: "incident-writeup", required: true, why: "Writes the incident summary." }]);
    expect(names(files()).some((n) => n.startsWith("pr-triage/skills/incident-writeup"))).toBe(false);
  });

  it("writes apps by level, and custom as tools", () => {
    const fs = templateFiles(
      exp,
      meta,
      [
        { app: "github", required: true, level: "act", why: "Merges." },
        { app: "linear", required: false, level: "custom", tools: { allow: ["linear_issue_get"], ask: ["linear_issue_create"] }, why: "Files issues." },
      ],
      refs,
      helpers,
      now,
    );
    expect((parseYaml(asText(fs, "pr-triage/template.yaml")) as Record<string, unknown>).apps).toEqual([
      { app: "github", required: true, level: "act", why: "Merges." },
      { app: "linear", required: false, tools: { allow: ["linear_issue_get"], ask: ["linear_issue_create"] }, why: "Files issues." },
    ]);
  });

  it("omits apps and skills from template.yaml when there are none", () => {
    const yaml = parseYaml(asText(templateFiles(exp, meta, [], [], [], now), "pr-triage/template.yaml")) as Record<string, unknown>;
    expect(yaml).not.toHaveProperty("apps");
    expect(yaml).not.toHaveProperty("skills");
    expect(yaml).toMatchObject({ format: 1, id: "pr-triage", version: "1.0.0", name: "PR triage", summary: "Triage the pull-request queue." });
  });

  it("gives the helpers their executable bit and nothing else one", () => {
    const a = attrs(zipBytes());
    const EXEC = (0o100755 << 16) >>> 0;
    expect(a.get("pr-triage/helpers/scripts/run.sh")).toBe(EXEC);
    for (const [name, value] of a) {
      if (name !== "pr-triage/helpers/scripts/run.sh") expect(value).toBe(0);
    }
  });

  it("README.md is the What it's for text plus the closing line", () => {
    expect(asText(files(), "pr-triage/README.md")).toBe(
      "## What Daneel does\n\nTriage.\n\nMade with Save as template on 2026-11-21. Accounts, secrets and conversations are not included.\n",
    );
  });

  it("never writes a connection id, account label, secret, webhook address, library token or model name", () => {
    const raw = new TextDecoder().decode(zipBytes());
    for (const leak of ["conn-123", "Acme GitHub account", "whsec_", "https://aurora.test/hooks/", "glpat-", "claude-sonnet-4", "Europe/Zurich"]) {
      expect(raw).not.toContain(leak);
    }
  });

  it("refuses a placeholder in the texts that would be filled in, naming the file", () => {
    expect(() => files({ ...exp, instructions: "Use {input.jira_project}." })).toThrow(/instructions\.md/);
    expect(() => files({ ...exp, memories: [{ key: "k", updated: "", content: "Hello {person.name}." }] })).toThrow(/memory\/k\.md/);
    expect(() => files({ ...exp, skills: [{ ...exp.skills[0]!, body: "Ask {agent.name}." }] })).toThrow(/SKILL\.md/);
    expect(() =>
      files({ ...exp, automations: [{ kind: "schedule", name: "S", prompt: "Ping {agent.name}.", cadence: { cron: "0 8 * * *" }, time_zone: "UTC", enabled: true }] }),
    ).toThrow(/morning\.yaml|s\.yaml|automations\//);
  });

  it("refuses an executable helper that is not UTF-8 text", () => {
    expect(() => templateFiles(exp, meta, apps, refs, [{ path: "run.sh", content: new Uint8Array([0xff, 0xfe]), executable: true }], now)).toThrow(/run\.sh.*must be text/);
    // A non-executable helper may be binary.
    expect(() => templateFiles(exp, meta, apps, refs, [{ path: "blob.bin", content: new Uint8Array([0xff, 0xfe]), executable: false }], now)).not.toThrow();
  });
});

describe("templateYaml", () => {
  it("is strict-shape: only the keys the Rust parser knows", () => {
    expect(Object.keys(templateYaml(meta, apps, refs) as Record<string, unknown>).sort()).toEqual(["apps", "format", "id", "name", "skills", "summary", "version"]);
  });

  it("carries a reference's ref and path when set", () => {
    const yaml = templateYaml(meta, [], [{ ...refs[0]!, ref: "v1.2", path: "skills" }]) as Record<string, unknown>;
    expect(yaml.skills).toEqual([
      { library: "https://gitlab.example.org/agents/skills.git", ref: "v1.2", path: "skills", skill: "incident-writeup", required: true, why: "Writes the incident summary." },
    ]);
  });
});

describe("placeholderProblem", () => {
  it("names the placeholder, and nothing else in braces", () => {
    expect(placeholderProblem("Use {input.jira_project} here")).toBe("it contains the placeholder “{input.jira_project}”, which a template fills in");
    for (const t of ["{agent.name}", "{person.name}"]) expect(placeholderProblem(t)).not.toBeNull();
    expect(placeholderProblem("keep {literal} and { Input.x }")).toBeNull();
  });
});

describe("appAccessRows", () => {
  const acc = (app: string | null, level: TemplateAccountRow["agents"][number]["level"], refId = "c1"): TemplateAccountRow => ({
    kind: "connection",
    app: app ? { app_id: app } : null,
    ref: { type: "connection", id: refId },
    agents: [{ agent: { id: "a1" }, level }],
  });
  const grant = (connectionId: string, mode: TemplateGrantRow["mode"], allow: string[] = [], ask: string[] = []): TemplateGrantRow => ({
    agent_id: "a1",
    connection_id: connectionId,
    mode,
    allow_tools: allow,
    ask_tools: ask,
  });

  it("maps the classified levels straight through", () => {
    expect(appAccessRows([acc("github", "read"), acc("linear", "ask"), acc("atlassian", "act")], [], "a1")).toEqual([
      { app: "atlassian", level: "act" },
      { app: "github", level: "read" },
      { app: "linear", level: "ask" },
    ]);
  });

  it("takes a custom row's tool lists from the agent's grant", () => {
    expect(appAccessRows([acc("github", "custom", "c9")], [grant("c9", "custom", ["jira_search"], ["jira_issue_create"])], "a1")).toEqual([
      { app: "github", level: "custom", tools: { allow: ["jira_search"], ask: ["jira_issue_create"] } },
    ]);
  });

  it("reads a full grant as act, and skips a custom row without a grant", () => {
    expect(appAccessRows([acc("github", "custom", "c1"), acc("linear", "use", "c2")], [grant("c2", "full")], "a1")).toEqual([{ app: "linear", level: "act" }]);
  });

  it("merges several accounts of one app, strongest level wins, and skips what is not an app", () => {
    expect(appAccessRows([acc("github", "read", "c1"), acc("github", "act", "c2"), { ...acc(null, "use"), kind: "ai_key" }], [], "a1")).toEqual([{ app: "github", level: "act" }]);
  });

  it("ignores accounts the agent has no row for", () => {
    expect(appAccessRows([{ ...acc("github", "act"), agents: [] }], [], "a1")).toEqual([]);
  });
});
