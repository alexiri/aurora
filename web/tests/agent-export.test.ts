import { afterEach, describe, expect, it, vi } from "vitest";

import { exportAsTemplate, exportFiles, exportZip, ExportError, parseExport, slug, templateAsExport, type AgentExport } from "@/lib/export/agent-export";
import { loadAgentExport } from "@/lib/export/agent-import";
import { readZip, writeZip } from "@/lib/export/zip";
import { parseYaml } from "@/lib/export/yaml";
import { AGENT_TEMPLATES } from "@/lib/onboarding/templates";
import { json, mockFetch } from "./support/api";

// P5-REFINE-19 item 4: what an export holds (the tree, no secrets) and that a template is an export with `purpose` set.

afterEach(() => vi.unstubAllGlobals());

const text = (b: Uint8Array) => new TextDecoder().decode(b);
const NOW = new Date("2026-11-20T10:00:00Z");

const sample: AgentExport = {
  format: 1,
  name: "Daneel",
  purpose: "research",
  model: "default",
  may_message_first: true,
  instructions: "You are Daneel.\nBe brief.",
  memories: [
    { key: "likes-tea", updated: "2026-11-01T09:00:00Z", content: "Ada likes tea.\n---\nnot a front matter end" },
    { key: "timezone", updated: "2026-11-02T09:00:00Z", content: "Zurich" },
  ],
  skills: [{ name: "release-notes", useItWhen: "Someone asks: what shipped?", body: "# Release notes\nSteps…", files: [] }],
  automations: [
    { kind: "schedule", name: "Morning check", prompt: "Check the news.", cadence: { cron: "0 8 * * 1-5" }, time_zone: "Europe/Zurich", enabled: true },
    { kind: "webhook", name: "Deploy notice", instructions: "Say what shipped.", verification: "signed", enabled: false },
  ],
};

describe("the tree", () => {
  it("writes agent.yaml, instructions.md, memory, skills, automations and the README under <handle>/", () => {
    const names = exportFiles(sample, "daneel", NOW).map((f) => f.name);
    expect(names).toEqual([
      "daneel/agent.yaml",
      "daneel/instructions.md",
      "daneel/memory/likes-tea.md",
      "daneel/memory/timezone.md",
      "daneel/skills/release-notes/SKILL.md",
      "daneel/automations/morning-check.yaml",
      "daneel/automations/deploy-notice.yaml",
      "daneel/README.md",
    ]);
  });

  it("names the zip <handle>-<date>.zip", () => {
    expect(exportZip(sample, "Daneel", NOW).fileName).toBe("daneel-2026-11-20.zip");
  });

  it("has the documented agent.yaml and README", () => {
    const files = Object.fromEntries(exportFiles(sample, "daneel", NOW).map((f) => [f.name, text(f.data)]));
    expect(files["daneel/agent.yaml"]).toBe("format: 1\nname: Daneel\npurpose: research\nmodel: default\nmay_message_first: true\n");
    expect(files["daneel/README.md"]).toBe("Exported from Aurora on 2026-11-20. Secrets, accounts and conversations are not included.\n");
    expect(files["daneel/memory/likes-tea.md"]).toMatch(/^---\nkey: likes-tea\nupdated: "2026-11-01T09:00:00Z"\n---\nAda likes tea\./);
    expect(files["daneel/skills/release-notes/SKILL.md"]).toMatch(/^---\nname: release-notes\nuse-it-when: "Someone asks: what shipped\?"\n---\n# Release notes/);
    expect(parseYaml(files["daneel/automations/morning-check.yaml"]!)).toEqual({
      kind: "schedule", name: "Morning check", prompt: "Check the news.", cadence: { cron: "0 8 * * 1-5" }, time_zone: "Europe/Zurich", enabled: true,
    });
  });

  it("round-trips through a zip, content and all", async () => {
    const back = parseExport(await readZip(exportZip(sample, "daneel", NOW).bytes));
    expect(back).toEqual(sample);
  });

  it("slugs names for files", () => {
    expect(slug("Morning check!")).toBe("morning-check");
    expect(slug("Café déjà vu")).toBe("cafe-deja-vu");
    expect(slug("***")).toBe("item");
  });
});

describe("reading refuses what it should", () => {
  const enc = (s: string) => new TextEncoder().encode(s);
  const zip = (files: Record<string, string>) => Promise.resolve(Object.entries(files).map(([name, c]) => ({ name, data: enc(c) })));

  it("a zip with no agent folder or no agent.yaml", async () => {
    expect(() => parseExport([{ name: "a.txt", data: enc("x") }])).toThrow(ExportError);
    expect(() => parseExport([{ name: "x/readme.md", data: enc("x") }])).toThrow(/agent\.yaml is missing/);
    expect(() => parseExport([{ name: "a/agent.yaml", data: enc("format: 1\nname: A\n") }, { name: "b/agent.yaml", data: enc("x") }])).toThrow(/single agent folder/);
  });

  it("a newer format and a bad key", async () => {
    expect(() => parseExport([{ name: "a/agent.yaml", data: enc("format: 2\nname: A\n") }])).toThrow(/newer Aurora/);
    const files = await zip({ "a/agent.yaml": "format: 1\nname: A\n", "a/memory/x.md": '---\nkey: "Not Kebab"\n---\nhi' });
    expect(() => parseExport(files)).toThrow(/kebab-case key/);
  });

  it("YAML with an anchor, a tag or an alias", async () => {
    for (const bad of ["format: 1\nname: &a A\n", "format: 1\nname: !!str A\n", "format: 1\nname: *a\n"]) {
      expect(() => parseExport([{ name: "a/agent.yaml", data: enc(bad) }]), bad).toThrow(ExportError);
    }
  });

  it("an automation of an unknown kind or verification", async () => {
    const base = { "a/agent.yaml": "format: 1\nname: A\n" };
    expect(() => parseExport([...Object.entries({ ...base, "a/automations/x.yaml": "kind: cron\nname: x\n" }).map(([name, c]) => ({ name, data: enc(c) }))])).toThrow(/kind is schedule or webhook/);
    expect(() =>
      parseExport(Object.entries({ ...base, "a/automations/x.yaml": "kind: webhook\nname: x\nverification: magic\n" }).map(([name, c]) => ({ name, data: enc(c) }))),
    ).toThrow(/signed, password or none/);
  });

  it("a hand-made zip with a name that climbs out never gets as far as parsing", async () => {
    expect(() => writeZip([{ name: "a/../../x.md", data: enc("x") }])).toThrow();
  });
});

describe("a template is an export with purpose set", () => {
  it("every template survives export, zip and read as itself", async () => {
    for (const t of AGENT_TEMPLATES) {
      const exp = templateAsExport(t);
      expect(exp.purpose).toBe(t.id);
      const back = parseExport(await readZip(exportZip(exp, t.nameSuggestion || "blank", NOW).bytes));
      expect(back).toEqual(exp);
      const asTemplate = exportAsTemplate(back, t.label);
      expect(asTemplate).toEqual({
        id: t.id,
        label: t.label,
        nameSuggestion: t.nameSuggestion,
        instructions: t.instructions,
        ...(t.schedule ? { schedule: t.schedule } : {}),
      });
    }
  });

  it("an export built from the agent's own data equals the template's export", async () => {
    const t = AGENT_TEMPLATES.find((x) => x.schedule)!;
    mockFetch({
      "GET /v1/agents/a1/memories": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/agents/a1/skills": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/agents/a1/schedules": () =>
        json(200, { items: [{ id: "s1", name: t.label, prompt: t.schedule!.prompt, cron: t.schedule!.cron, timezone: "UTC", enabled: false }], next_cursor: null }),
      "GET /v1/agents/a1/webhooks": () => json(200, { items: [], next_cursor: null }),
    });
    const { data } = await loadAgentExport({ id: "a1", name: t.nameSuggestion, brief: t.instructions, model_id: null, proactive_messaging: "ask" });
    expect({ ...data, purpose: t.id }).toEqual(templateAsExport(t));
  });
});

describe("what never leaves", () => {
  it("no id, address, secret or account is in the zip", async () => {
    const hook = {
      id: "6f1c2f6a-1111-4222-8333-444455556666", agent_id: "a1", name: "Deploy notice", instructions: "Say what shipped.", enabled: true,
      url_prefix: "https://aurora.test/hooks/SECRETPREFIX9876", verification: { kind: "hmac_sha256", header: "X-Signature", format: "sha256=<hex>" },
      anchor: "workspace", created_by: { id: "u-777", display_name: "Ada Lovelace", email: "ada@example.test" }, last_delivery_at: null,
    };
    mockFetch({
      "GET /v1/agents/a1/memories": () => json(200, { items: [{ key: "likes-tea", content: "Ada likes tea.", index_line: "x", revision: 3, updated_at: "2026-11-01T09:00:00Z", created_at: "2026-10-01T09:00:00Z" }], next_cursor: null }),
      "GET /v1/agents/a1/skills": () =>
        json(200, { items: [{ id: "sk-123456", name: "release-notes", description: "When asked what shipped", body: "# Notes", files: [{ path: "SKILL.md", size: 5, executable: false }, { path: "run.sh", size: 9, executable: true }], org_id: "org-999" }], next_cursor: null }),
      "GET /v1/agents/a1/schedules": () =>
        json(200, { items: [
          { id: "sched-5", agent_id: "a1", name: "Morning", prompt: "Check.", cron: "0 8 * * *", timezone: "UTC", enabled: true, created_by: { id: "u-777" } },
          { id: "sched-6", agent_id: "a1", name: "Once", prompt: "Later.", cron: null, run_at: "2026-12-01T09:00:00Z", timezone: "UTC", enabled: true },
        ], next_cursor: null }),
      "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook], next_cursor: null }),
    });
    const { data, omitted } = await loadAgentExport({ id: "a1", name: "Daneel", brief: "You are Daneel.", model_id: "claude-sonnet", proactive_messaging: "allow" });
    expect(data.model).toBe("claude-sonnet");
    expect(data.may_message_first).toBe(true);
    // A one-time schedule has no cadence; skill files other than SKILL.md cannot be read back: both are said, not silently dropped.
    expect(omitted).toEqual(["The one-time schedule “Once” is not exported.", "Skill files other than SKILL.md are not exported: the API cannot read them back."]);
    const files = exportFiles(data, "daneel", NOW);
    const all = files.map((f) => `${f.name}\n${text(f.data)}`).join("\n=====\n");
    for (const leak of ["6f1c2f6a", "SECRETPREFIX", "hooks/", "u-777", "ada@example.test", "org-999", "sk-123456", "sched-5", "X-Signature", "https://"]) {
      expect(all, leak).not.toContain(leak);
    }
    // The zip itself, bytes and all.
    const zipText = text(exportZip(data, "daneel", NOW).bytes);
    expect(zipText).not.toContain("SECRETPREFIX");
    expect(zipText).not.toContain("6f1c2f6a");
  });
});
