import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { templateZip, type TemplateAppEntry, type TemplateHelper, type TemplateMeta, type TemplateSkillRef } from "@/lib/export/agent-template";
import type { AgentExport } from "@/lib/export/agent-export";

// P6-TPL-5: the golden fixture of the Rust reader's interop test
// (`crates/aurora-template/tests/web_saved.rs`): a zip the dialog's builder produced, with the
// shapes the reader must accept — two apps (one `act`, one custom with `tools:`), a library skill
// reference, a locked memory, a cron schedule and an executable helper.
//
// The builder is deterministic (the zip's DOS timestamp is the fixed 1980-01-01 the Rust `pack`
// writes, and the date pinned here only fills the README's "Made ... on <date>" line), so the
// committed fixture can be proved fresh: with `AURORA_TPL_FIXTURE=1` this test (re)writes
// `crates/aurora-template/tests/fixtures/web-saved-template.zip`; without it, the test rebuilds
// the zip and compares it with the committed file byte for byte.
//
// The time zone matters: the zip writer used to stamp entries with the machine's LOCAL time
// (`dosDateTime` reads local getters), so the bytes — and this byte-for-byte comparison against
// the committed fixture — differed between CI (UTC) and a developer machine (CET). The fixed
// timestamp makes the archive identical in every time zone; this test is run under TZ=UTC,
// TZ=Europe/Zurich and TZ=Pacific/Kiritimati to prove it.

const FIXTURE = join(__dirname, "..", "..", "crates", "aurora-template", "tests", "fixtures", "web-saved-template.zip");

const e: AgentExport = {
  format: 1,
  name: "Daneel",
  model: "default",
  may_message_first: true,
  instructions: "Triage the team's pull requests and ask before anything that writes.",
  memories: [{ key: "queue-notes", updated: "2026-11-01T09:00:00Z", content: "The team's queue drains after standup.", locked: true }],
  skills: [{ name: "release-notes", useItWhen: "When asked what shipped", body: "# Release notes\n\nWrite the notes from the merged pull requests.", files: [] }],
  automations: [{ kind: "schedule", name: "Morning", prompt: "Check the triage queue.", cadence: { cron: "0 8 * * 1-5" }, time_zone: "UTC", enabled: true }],
};

const meta: TemplateMeta = {
  id: "daneel",
  version: "1.0.0",
  name: "Daneel",
  summary: "Triage the team's pull requests.",
  readme: "## What Daneel does\n\nTriage pull requests and hand them to the right reviewer.\n",
};

const apps: TemplateAppEntry[] = [
  { app: "github", required: true, level: "act", why: "Reads and triages the pull requests." },
  { app: "linear", required: false, level: "custom", tools: { allow: ["linear_issue_get"], ask: ["linear_issue_create"] }, why: "Files the triage tickets." },
];

const refs: TemplateSkillRef[] = [
  { library: "https://gitlab.example.org/agents/skills.git", ref: "v1.2", path: "skills", skill: "incident-writeup", required: true, why: "Writes the incident summary." },
];

const helpers: TemplateHelper[] = [{ path: "tidy.sh", content: new TextEncoder().encode("#!/bin/sh\necho tidied\n"), executable: true }];

function build(): Uint8Array {
  return templateZip(e, meta, apps, refs, helpers, new Date("2026-11-24T09:00:00Z")).bytes;
}

describe("the web-saved-template golden fixture", () => {
  it("is what the dialog's builder produces", () => {
    const bytes = build();
    if (process.env.AURORA_TPL_FIXTURE === "1") {
      writeFileSync(FIXTURE, bytes);
      console.log(`wrote ${FIXTURE} (${bytes.length} bytes)`);
      return;
    }
    expect(existsSync(FIXTURE), "run AURORA_TPL_FIXTURE=1 pnpm exec vitest run tests/template-fixture.test.ts once").toBe(true);
    expect(Buffer.from(bytes).equals(readFileSync(FIXTURE))).toBe(true);
  });
});
