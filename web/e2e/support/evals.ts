import { expect, type Page } from "@playwright/test";

import { agent, id, mockApi, now } from "./mock-api";
import type { Scheme } from "./open";

// P4-EVAL-4: the agent's Evals page (Behaviour section): questions, judge, runs, schedules, a run's page with the
// configuration difference, and the two dialogs. The API is the shared mock plus these routes (mocked mode: no server).

export const EVAL_SCREENS = ["page", "run", "question-dialog", "schedule-dialog"] as const;
export type EvalScreen = (typeof EVAL_SCREENS)[number];

const ref = { id: agent.id, name: agent.name };
const q = (n: number, over: { question: string; expected_kind: string } & Record<string, unknown>) => ({
  id: id(700 + n), agent: ref, position: n, enabled: true, version: 1, created_at: now, updated_at: now,
  expected_number: null, tolerance: null, expected_text: null, criteria: null, ...over,
});
const questions = [
  q(1, { question: "How many open pull requests does the repository have right now?", expected_kind: "numeric", expected_number: 42, tolerance: 2 }),
  q(2, { question: "What is the name of the team's release branch?", expected_kind: "text", expected_text: "release/2026" }),
  q(3, { question: "Explain in two sentences why the nightly build failed yesterday.", expected_kind: "judged", criteria: "Names the failing test and says whether a retry would help." }),
];

const counts = (passed: number, failed: number, errored: number, notScored: number) => ({ total: 3, passed, failed, errored, not_scored: notScored, refused_writes: 1 });
const run = (n: number, over: Record<string, unknown>) => ({
  id: id(800 + n), agent: ref, trigger: "manual", schedule: null, started_by: { id: id(99), label: "Ada Lovelace" }, status: "done", error_code: null,
  question_set_version: "c".repeat(64), fingerprint_hash: String(n).repeat(64), counts: counts(2, 1, 0, 0), pass_rate: 66.7, judge_usage: null, regression: null,
  created_at: now, started_at: now, finished_at: now, ...over,
});
const runs = [
  run(2, { regression: { from_run_id: id(801), drop_points: 12.5 }, pass_rate: 50, counts: counts(1, 1, 1, 0) }),
  run(1, { pass_rate: 100, counts: counts(3, 0, 0, 0), trigger: "schedule", schedule: { id: id(900), name: "Every morning" }, started_by: null }),
];
const fingerprint = (model: string, instructions: string) => ({
  instructions_sha256: instructions.repeat(64), harness: "claude-code", model: { provider: "anthropic", endpoint_id: "anthropic", model_id: model }, effort: "medium",
  model_roles: { large: null, medium: null, small: null }, subagent_model: "inherit", skills: [{ tier: "agent", name: "release-notes", content_sha256: "d".repeat(64) }],
  granted_rule_ids: [id(950)], judge: null, question_set_version: "c".repeat(64),
});
const result = (n: number, over: Record<string, unknown>) => ({
  position: n, question: { id: id(700 + n), text: questions[n - 1]!.question }, expected_kind: questions[n - 1]!.expected_kind, expected: { number: null, tolerance: null, text: null, criteria: null },
  outcome: "pass", reason_code: null, extracted_value: null, answer: "ANSWER: 42", judge: null, refused_writes: 0, conversation_id: id(600 + n), started_at: now, finished_at: now, ...over,
});
const detail = (r: ReturnType<typeof run>, model: string, instructions: string) => ({
  ...r, fingerprint: fingerprint(model, instructions), next_results_after: null,
  results: [
    result(1, { expected: { number: 42, tolerance: 2, text: null, criteria: null }, answer: "There are 41 open pull requests.\nANSWER: 41", extracted_value: "41" }),
    result(2, { outcome: "fail", reason_code: "no_answer_line", answer: "I believe it is the release branch for the year.", expected: { number: null, tolerance: null, text: "release/2026", criteria: null }, refused_writes: 1 }),
    result(3, { outcome: "error", reason_code: "judge_not_available", answer: "The nightly build failed because a test timed out; a retry would not help.", expected: { number: null, tolerance: null, text: null, criteria: "Names the failing test" } }),
  ],
});
const schedules = [
  { id: id(900), agent: ref, name: "Every morning", cron: "0 6 * * *", timezone: "UTC", enabled: true, anchor: "creator", creator: { id: id(99), label: "Ada Lovelace" }, disabled_reason: null, next_fire_at: now, last_fire_at: now, last_outcome: "ok", last_run_id: id(801) },
  { id: id(901), agent: ref, name: "Weekly check", cron: "30 9 * * 1", timezone: "Europe/Zurich", enabled: false, anchor: "workspace", creator: null, disabled_reason: "failures", next_fire_at: null, last_fire_at: now, last_outcome: "failed", last_run_id: null },
];

/** Mock routes for the Evals page; later routes win over the shared mock, the rest falls through to it. */
export async function mockEvals(page: Page) {
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    const base = `/v1/agents/${agent.id}`;
    if (p === `${base}/eval-questions`) return json({ questions });
    if (p === `${base}/eval-settings`) return json({ judge: { secret_id: id(60), secret_label: "Org AI key", model_id: "claude-sonnet-5-5", usable: true }, regression_notify: true });
    if (p === `${base}/eval-runs`) return json({ items: runs, next_cursor: null });
    if (p === `${base}/eval-schedules`) return json({ schedules });
    if (p === `/v1/eval-runs/${runs[0]!.id}`) return json(detail(runs[0]!, "claude-sonnet-5-5", "b"));
    if (p === `/v1/eval-runs/${runs[1]!.id}`) return json(detail(runs[1]!, "claude-opus-5-5", "a"));
    if (p === `${base}/model-readiness`)
      return json({ harness: "claude-code", override_dropped: false, default: null, keys: [{ secret_id: id(60), name: "Org AI key", scope: "organization", provider: "anthropic", endpoint_id: "anthropic", usable: true, reason: null }] });
    return route.fallback();
  });
}

/** Opens `?tab=evals` on the mocked agent: the page, a run's page (compared with the previous run), or a dialog. */
export async function openEvals(page: Page, screen: EvalScreen, scheme: Scheme) {
  await mockEvals(page);
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/o/acme/w/main/agents/daneel?tab=evals");
  await page.getByRole("heading", { name: "Questions", level: 2 }).waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  if (screen === "run") {
    // The newest run has a regression notice; "See what changed" opens it against the run it dropped from.
    await page.getByRole("button", { name: "See what changed" }).click();
    await page.getByRole("heading", { name: "What changed", level: 2 }).waitFor();
    // The table (from md) or the stacked list (a phone): whichever is on screen.
    await page.locator("text=claude-opus-5-5 >> visible=true").first().waitFor();
    await page.waitForLoadState("networkidle");
  }
  if (screen === "question-dialog") {
    await page.getByRole("button", { name: "Edit question 1" }).click();
    await page.getByRole("dialog").waitFor();
  }
  if (screen === "schedule-dialog") {
    await page.getByRole("button", { name: "New schedule" }).click();
    await page.getByRole("dialog").waitFor();
  }
  expect(errors, "the Evals page threw").toEqual([]);
}
