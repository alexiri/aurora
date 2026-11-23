import { api } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { readSkillFile } from "@/lib/api/skills";
import { unwrap } from "@/lib/api/unwrap";
import type { Agent } from "@/lib/api/queries";
import type { components } from "@/lib/api/schema";

import { EXPORT_FORMAT, type AgentExport, type ExportAutomation, type ExportMemory, type ExportSkill } from "./agent-export";

/**
 * (P5-REFINE-19, D-413) Reading an agent out of the existing routes and putting an export back through them. Nothing is
 * uploaded as a file: the zip is read in the browser and each item goes through the route that creates it, so every
 * server-side check (names, limits, roles) applies. Import never changes access, accounts or the AI key.
 */

type S = components["schemas"];

// ---- export: read the agent ---------------------------------------------------------------------------------------

/** The agent as an export: its own settings, instructions, memories, skills, schedules and webhooks (no ids, no secrets). */
export async function loadAgentExport(agent: Pick<Agent, "id" | "name" | "brief" | "model_id" | "proactive_messaging">): Promise<{ data: AgentExport; omitted: string[] }> {
  const path = { params: { path: { agent: agent.id } } };
  const [memories, skills, schedules, webhooks] = await Promise.all([
    unwrap(api.GET("/v1/agents/{agent}/memories", path)),
    unwrap(api.GET("/v1/agents/{agent}/skills", path)),
    unwrap(api.GET("/v1/agents/{agent}/schedules", path)),
    unwrap(api.GET("/v1/agents/{agent}/webhooks", path)),
  ]);
  const omitted: string[] = [];
  const automations: ExportAutomation[] = [];
  for (const s of schedules.items) {
    if (s.cron) {
      automations.push({ kind: "schedule", name: s.name, prompt: s.prompt, cadence: { cron: s.cron }, time_zone: s.timezone, enabled: s.enabled });
    } else if (s.completed_at || !s.run_at) {
      // A one-time schedule that already ran is Done: there is nothing left to carry.
      omitted.push(`The one-time schedule “${s.name}” already ran, so it is not exported.`);
    } else {
      // A pending one-time schedule: its local time and zone (C-1092).
      automations.push({ kind: "schedule", name: s.name, prompt: s.prompt, cadence: { preset: "once", run_at: s.run_at }, time_zone: s.timezone, enabled: s.enabled });
    }
  }
  for (const w of webhooks.items) {
    const kind = w.verification.kind;
    automations.push({ kind: "webhook", name: w.name, instructions: w.instructions, verification: kind === "hmac_sha256" ? "signed" : kind === "bearer" ? "password" : "none", enabled: w.enabled });
  }
  // The skill's other files, as stored (C-1099); the list is on the skill itself.
  const skillList: ExportSkill[] = await Promise.all(
    skills.items.map(async (s) => ({
      name: s.name,
      useItWhen: s.description,
      body: s.body,
      files: await Promise.all(s.files.map(async (f) => ({ path: f.path, content: await readSkillFile(s.id, f.path) }))),
    })),
  );
  return {
    omitted,
    data: {
      format: EXPORT_FORMAT,
      name: agent.name,
      model: agent.model_id ?? "default",
      may_message_first: agent.proactive_messaging === "allow",
      instructions: agent.brief,
      memories: memories.items.map((m) => ({ key: m.key, updated: m.updated_at, content: m.content, ...(m.locked ? { locked: true } : {}) })),
      skills: skillList,
      automations,
    },
  };
}

/** What the agent has now, for the skip rule of an import. */
export async function loadExisting(agent: Pick<Agent, "id" | "brief">): Promise<ImportExisting> {
  const path = { params: { path: { agent: agent.id } } };
  const [memories, skills, schedules, webhooks] = await Promise.all([
    unwrap(api.GET("/v1/agents/{agent}/memories", path)),
    unwrap(api.GET("/v1/agents/{agent}/skills", path)),
    unwrap(api.GET("/v1/agents/{agent}/schedules", path)),
    unwrap(api.GET("/v1/agents/{agent}/webhooks", path)),
  ]);
  return {
    instructions: agent.brief,
    memoryKeys: memories.items.map((m) => m.key),
    skillNames: skills.items.map((s) => s.name),
    scheduleNames: schedules.items.map((s) => s.name),
    webhookNames: webhooks.items.map((w) => w.name),
  };
}

// ---- import --------------------------------------------------------------------------------------------------------

export interface ImportChoices {
  instructions: boolean;
  /** `keep` leaves the agent's own instructions; `replace` writes the export's. */
  instructionsMode: "keep" | "replace";
  memories: boolean;
  skills: boolean;
  schedules: boolean;
  webhooks: boolean;
}

export const DEFAULT_CHOICES: ImportChoices = { instructions: true, instructionsMode: "keep", memories: true, skills: true, schedules: true, webhooks: true };

/** What the agent has already: items with these names are skipped, never overwritten. */
export interface ImportExisting {
  instructions: string;
  memoryKeys: string[];
  skillNames: string[];
  scheduleNames: string[];
  webhookNames: string[];
}

/** The routes an import uses, one call each; tests pass a fake. */
export interface ImportApi {
  setInstructions(text: string): Promise<void>;
  createMemory(m: ExportMemory): Promise<void>;
  createSkill(s: ExportSkill): Promise<{ id: string }>;
  putSkillFile(skillId: string, path: string, content: Uint8Array): Promise<void>;
  createSchedule(a: Extract<ExportAutomation, { kind: "schedule" }>): Promise<{ id: string }>;
  pauseSchedule(id: string): Promise<void>;
  createWebhook(a: Extract<ExportAutomation, { kind: "webhook" }>): Promise<{ id: string; secret: string | null }>;
  pauseWebhook(id: string): Promise<void>;
}

export interface ImportReport {
  done: string[];
  skipped: string[];
  failed: string[];
  /** A signing secret the server generated, shown once by the caller. */
  secrets: { name: string; secret: string }[];
  schedulesPaused: number;
  webhooksPaused: number;
}

/** How many of each group an export holds, for the checklist ("Instructions · 3 memories (1 locked) · 2 skills · 1 schedule · 1 webhook"). */
export function summarize(e: AgentExport): { instructions: boolean; memories: number; locked: number; skills: number; schedules: number; webhooks: number } {
  return {
    instructions: e.instructions.trim() !== "",
    memories: e.memories.length,
    // (P6-MEMLOCK, C-1150) The checklist says how many memories are locked.
    locked: e.memories.filter((m) => m.locked).length,
    skills: e.skills.length,
    schedules: e.automations.filter((a) => a.kind === "schedule").length,
    webhooks: e.automations.filter((a) => a.kind === "webhook").length,
  };
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The checklist line. */
export function summaryLine(e: AgentExport): string {
  const s = summarize(e);
  const memories = s.memories ? `${plural(s.memories, "memory", "memories")}${s.locked ? ` (${s.locked} locked)` : ""}` : null;
  return [s.instructions ? "Instructions" : null, memories, s.skills ? plural(s.skills, "skill") : null, s.schedules ? plural(s.schedules, "schedule") : null, s.webhooks ? plural(s.webhooks, "webhook") : null]
    .filter((x): x is string => x !== null)
    .join(" · ");
}

/**
 * Creates what the person ticked. An item whose name exists is skipped and listed; a schedule or webhook is created and then
 * paused (a webhook gets a fresh signing secret, returned in `secrets`); one that fails is listed and the rest go on.
 * Successes are not rolled back.
 */
export async function runImport(e: AgentExport, choices: ImportChoices, existing: ImportExisting, io: ImportApi, onProgress?: (done: number, total: number) => void): Promise<ImportReport> {
  const report: ImportReport = { done: [], skipped: [], failed: [], secrets: [], schedulesPaused: 0, webhooksPaused: 0 };
  const memories = choices.memories ? e.memories : [];
  const skills = choices.skills ? e.skills : [];
  const schedules = choices.schedules ? e.automations.filter((a): a is Extract<ExportAutomation, { kind: "schedule" }> => a.kind === "schedule") : [];
  const webhooks = choices.webhooks ? e.automations.filter((a): a is Extract<ExportAutomation, { kind: "webhook" }> => a.kind === "webhook") : [];
  const writeInstructions = choices.instructions && choices.instructionsMode === "replace" && e.instructions.trim() !== "";
  const total = (writeInstructions ? 1 : 0) + memories.length + skills.length + schedules.length + webhooks.length;
  let n = 0;
  const step = async (what: string, fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (err) {
      report.failed.push(`${what}: ${errorMessage(err)}`);
    }
    onProgress?.(++n, total);
  };

  if (writeInstructions) {
    await step("Instructions", async () => {
      await io.setInstructions(e.instructions);
      report.done.push("Instructions replaced");
    });
  }
  for (const m of memories) {
    if (existing.memoryKeys.includes(m.key)) {
      report.skipped.push(`Skipped: memory “${m.key}” already exists`);
      onProgress?.(++n, total);
      continue;
    }
    await step(`Memory “${m.key}”`, async () => {
      await io.createMemory(m);
      report.done.push(`Memory “${m.key}”`);
    });
  }
  for (const s of skills) {
    if (existing.skillNames.includes(s.name)) {
      report.skipped.push(`Skipped: skill “${s.name}” already exists`);
      onProgress?.(++n, total);
      continue;
    }
    await step(`Skill “${s.name}”`, async () => {
      const made = await io.createSkill(s);
      for (const f of s.files) await io.putSkillFile(made.id, f.path, f.content);
      report.done.push(`Skill “${s.name}”`);
    });
  }
  for (const a of schedules) {
    if (existing.scheduleNames.includes(a.name)) {
      report.skipped.push(`Skipped: schedule “${a.name}” already exists`);
      onProgress?.(++n, total);
      continue;
    }
    await step(`Schedule “${a.name}”`, async () => {
      const made = await io.createSchedule(a);
      await io.pauseSchedule(made.id);
      report.schedulesPaused++;
      report.done.push(`Schedule “${a.name}” (paused)`);
    });
  }
  for (const a of webhooks) {
    if (existing.webhookNames.includes(a.name)) {
      report.skipped.push(`Skipped: webhook “${a.name}” already exists`);
      onProgress?.(++n, total);
      continue;
    }
    await step(`Webhook “${a.name}”`, async () => {
      const made = await io.createWebhook(a);
      if (made.secret) report.secrets.push({ name: a.name, secret: made.secret });
      await io.pauseWebhook(made.id);
      report.webhooksPaused++;
      report.done.push(`Webhook “${a.name}” (paused)`);
    });
  }
  return report;
}

/** The routes, for one agent. */
export function liveImportApi(agentId: string): ImportApi {
  const path = { params: { path: { agent: agentId } } };
  return {
    async setInstructions(text) {
      await unwrap(api.PATCH("/v1/agents/{agent}", { ...path, body: { brief: text } }));
    },
    async createMemory(m) {
      // (P6-MEMLOCK, C-1150) A locked memory arrives locked, `locked_by` the importer.
      await unwrap(api.POST("/v1/agents/{agent}/memories", { ...path, body: { key: m.key, content: m.content, ...(m.locked ? { locked: true } : {}) } }));
    },
    async createSkill(s) {
      const made = await unwrap(api.POST("/v1/agents/{agent}/skills", { ...path, body: { name: s.name, description: s.useItWhen, body: s.body } }));
      return { id: made.id };
    },
    async putSkillFile(id, filePath, content) {
      await unwrap(
        api.PUT("/v1/skills/{id}/files/{*path}", {
          params: { path: { id, "*path": filePath } as unknown as { id: string; path: string }, query: {} },
          body: content as unknown as number[],
          bodySerializer: (b: unknown) => b as BodyInit,
          headers: { "content-type": "application/octet-stream" },
        }),
      );
    },
    async createSchedule(a) {
      // Exactly one of `cron` / `run_at` (C-1092); a one-time schedule's `run_at` is its local time in `time_zone`.
      const when = "cron" in a.cadence ? { cron: a.cadence.cron } : { run_at: a.cadence.run_at };
      const made = await unwrap(api.POST("/v1/agents/{agent}/schedules", { ...path, body: { name: a.name, prompt: a.prompt, ...when, timezone: a.time_zone } satisfies S["CreateScheduleRequest"] }));
      return { id: made.id };
    },
    async pauseSchedule(id) {
      await unwrap(api.PATCH("/v1/schedules/{id}", { params: { path: { id } }, body: { enabled: false } }));
    },
    async createWebhook(a) {
      const verification = a.verification === "none" ? { kind: "none" } : { kind: a.verification === "signed" ? "hmac_sha256" : "bearer", generate_secret: true };
      const made = await unwrap(
        api.POST("/v1/agents/{agent}/webhooks", { ...path, body: { name: a.name, instructions: a.instructions.trim() || `Handle the “${a.name}” webhook.`, verification } satisfies S["CreateWebhookRequest"] }),
      );
      return { id: made.webhook.id, secret: made.secret ?? null };
    },
    async pauseWebhook(id) {
      await unwrap(api.PATCH("/v1/webhooks/{id}", { params: { path: { id } }, body: { enabled: false } }));
    },
  };
}
