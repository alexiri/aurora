import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { describeCron, firesMoreThanHourly } from "@/components/agents/evals/cron-words";
import { diffFingerprints } from "@/components/agents/evals/fingerprint-diff";
import { JudgeSection, JUDGE_OWNER_ONLY } from "@/components/agents/evals/judge-section";
import { QuestionsSection } from "@/components/agents/evals/questions-section";
import { RunDetail } from "@/components/agents/evals/run-detail";
import { RunsSection } from "@/components/agents/evals/runs-section";
import { startBlockedReason } from "@/components/agents/evals/run-state";
import { emptyDraft, parseNumber, toPatchBody, validateQuestion } from "@/components/agents/evals/question-form";
import { isPageAvailable } from "@/components/agents/sections";
import type { EvalQuestion, EvalRun } from "@/lib/api/evals";
import { wrap } from "./support/render";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

type Call = { method: string; path: string; body: unknown };

function mockFetch(routes: Record<string, () => Response>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const method = req?.method ?? "GET";
      const text = req && method !== "GET" && method !== "DELETE" ? await req.clone().text() : "";
      calls.push({ method, path: url.pathname, body: text ? JSON.parse(text) : null });
      const h = routes[`${method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

const A = "a1";

const question = (n: number, over: Partial<EvalQuestion> = {}): EvalQuestion => ({
  id: `q${n}`,
  agent: { id: A, name: "Rex" },
  position: n,
  question: `Question ${n}`,
  expected_kind: "numeric",
  expected_number: n,
  tolerance: 0,
  expected_text: null,
  criteria: null,
  enabled: true,
  version: 1,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  ...over,
});

const run = (id: string, over: Partial<EvalRun> = {}): EvalRun => ({
  id,
  agent: { id: A, name: "Rex" },
  trigger: "manual",
  schedule: null,
  started_by: { id: "u1", label: "Alice" },
  status: "done",
  error_code: null,
  question_set_version: "v1",
  fingerprint_hash: "a".repeat(64),
  counts: { total: 4, passed: 3, failed: 1, errored: 0, not_scored: 0, refused_writes: 0 },
  pass_rate: 75,
  judge_usage: null,
  regression: null,
  created_at: "2026-11-10T10:00:00Z",
  started_at: "2026-11-10T10:00:01Z",
  finished_at: "2026-11-10T10:05:00Z",
  ...over,
});

describe("question validation", () => {
  it("refuses a judged question without criteria and a numeric one without a number", () => {
    expect(validateQuestion({ ...emptyDraft, question: "Why?", kind: "judged", criteria: "  " }).criteria).toMatch(/Describe/);
    expect(validateQuestion({ ...emptyDraft, question: "Why?", kind: "judged", criteria: "Mentions X" })).toEqual({});
    expect(validateQuestion({ ...emptyDraft, question: "How many?", kind: "numeric", number: "forty" }).number).toMatch(/number/);
    expect(validateQuestion({ ...emptyDraft, question: "How many?", kind: "numeric", number: "42", tolerance: "-1" }).tolerance).toMatch(/zero or more/);
    expect(validateQuestion({ ...emptyDraft, question: "", kind: "numeric", number: "1" }).question).toBeDefined();
    expect(validateQuestion({ ...emptyDraft, question: "x".repeat(2001), kind: "numeric", number: "1" }).question).toMatch(/2000/);
    expect(validateQuestion({ ...emptyDraft, question: "Name?", kind: "text", text: "y".repeat(1001) }).text).toMatch(/1000/);
    expect(parseNumber("1e5")).toBeNull();
    expect(parseNumber("-3.5")).toBe(-3.5);
    expect(parseNumber("")).toBeNull();
  });

  it("sends a patch with only what changed", () => {
    const before = question(1, { expected_number: 42, tolerance: 0.5 });
    const draft = { ...emptyDraft, question: "Question 1", kind: "numeric" as const, number: "42", tolerance: "0.5" };
    expect(toPatchBody(draft, before)).toEqual({});
    expect(toPatchBody({ ...draft, number: "43" }, before)).toEqual({ expected_number: 43 });
  });

  it("shows the dialog's refusal and sends nothing while the form is invalid", async () => {
    const calls = mockFetch({ "GET /v1/agents/a1/eval-questions": () => json(200, { questions: [] }) });
    wrap(<QuestionsSection agentId={A} />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Add a question" }))[0]!);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByLabelText("Judged"));
    fireEvent.change(within(dialog).getByLabelText("Question"), { target: { value: "Explain it." } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add question" }));
    expect(await within(dialog).findByText(/Describe what a good answer contains/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });
});

describe("caps", () => {
  it("explains the 100-question cap and disables adding", async () => {
    mockFetch({ "GET /v1/agents/a1/eval-questions": () => json(200, { questions: Array.from({ length: 100 }, (_, i) => question(i + 1)) }) });
    wrap(<QuestionsSection agentId={A} />);
    expect(await screen.findByText(/This agent has 100 questions, the most it can have/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a question" })).toBeDisabled();
  });

  it("says why a run cannot start: active run, no questions, frozen, the day's limit", () => {
    const now = new Date("2026-11-17T12:00:00Z");
    const today = (i: number) => run(`r${i}`, { created_at: "2026-11-17T01:00:00Z" });
    expect(startBlockedReason([], { enabledQuestions: 2, frozen: false, now })).toBeNull();
    expect(startBlockedReason([], { enabledQuestions: 0, frozen: false, now })).toMatch(/at least one question/);
    expect(startBlockedReason([], { enabledQuestions: 2, frozen: true, now })).toMatch(/frozen/);
    expect(startBlockedReason([run("r", { status: "running" })], { enabledQuestions: 2, frozen: false, now })).toMatch(/in progress/);
    const twenty = Array.from({ length: 20 }, (_, i) => today(i));
    expect(startBlockedReason(twenty, { enabledQuestions: 2, frozen: false, now })).toMatch(/20 runs for today/);
    // Yesterday's runs do not count.
    const old = Array.from({ length: 20 }, (_, i) => run(`o${i}`, { created_at: "2026-11-16T23:00:00Z" }));
    expect(startBlockedReason(old, { enabledQuestions: 2, frozen: false, now })).toBeNull();
  });
});

describe("judge control", () => {
  const settings = { judge: { secret_id: "s1", secret_label: "Team key", model_id: "claude-x", usable: true }, regression_notify: true };

  it("is disabled for a member who does not own the workspace, with the reason", async () => {
    mockFetch({ "GET /v1/agents/a1/eval-settings": () => json(200, settings) });
    wrap(<JudgeSection agentId={A} canOwn={false} />);
    expect(await screen.findByText(JUDGE_OWNER_ONLY)).toBeInTheDocument();
    expect(screen.getByLabelText("Judge key")).toBeDisabled();
    expect(screen.getByLabelText("Judge model")).toBeDisabled();
    // The regression notice stays theirs to set.
    expect(screen.getByLabelText(/Show a notice when a run scores lower/)).not.toBeDisabled();
  });

  it("lets an owner choose a granted key and shows a refusal inline", async () => {
    const calls = mockFetch({
      "GET /v1/agents/a1/eval-settings": () => json(200, { judge: null, regression_notify: true }),
      "GET /v1/agents/a1/model-readiness": () =>
        json(200, { harness: "h", override_dropped: false, default: null, keys: [{ secret_id: "s9", name: "Org key", scope: "organization", provider: "p", endpoint_id: "e", usable: true, reason: null }] }),
      "PUT /v1/agents/a1/eval-settings": () => json(422, { error: { code: "eval_judge_not_granted", message: "x" } }),
    });
    wrap(<JudgeSection agentId={A} canOwn />);
    const select = await screen.findByLabelText("Judge key");
    await screen.findByRole("option", { name: /Org key/ });
    fireEvent.change(select, { target: { value: "s9" } });
    fireEvent.change(screen.getByLabelText("Judge model"), { target: { value: "m-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Save judge settings" }));
    expect(await screen.findByText(/must be an AI key that is granted to this agent/)).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ judge: { secret_id: "s9", model_id: "m-1" }, regression_notify: true });
  });
});

describe("fingerprint diff", () => {
  const base = {
    instructions_sha256: "1".repeat(64),
    harness: "claude-code",
    model: { provider: "anthropic", endpoint_id: "e1", model_id: "m1" },
    effort: "medium",
    model_roles: { large: null, medium: null, small: null },
    subagent_model: "inherit",
    skills: [{ tier: "agent", name: "notes", content_sha256: "2".repeat(64) }],
    granted_rule_ids: ["r1", "r2"],
    judge: null,
    question_set_version: "3".repeat(64),
  };

  it("lists exactly the changed fields, in words", () => {
    expect(diffFingerprints(base, structuredClone(base))).toEqual([]);
    const next = structuredClone(base);
    next.model.model_id = "m2";
    next.effort = "high";
    next.skills = [{ tier: "agent", name: "notes", content_sha256: "9".repeat(64) }, { tier: "agent", name: "new", content_sha256: "4".repeat(64) }];
    next.granted_rule_ids = ["r1", "r3"];
    next.instructions_sha256 = "5".repeat(64);
    const d = diffFingerprints(base, next);
    expect(d.map((c) => [c.field, c.before, c.after])).toEqual([
      ["Instructions (hash)", "1".repeat(64), "5".repeat(64)],
      ["Model", "m1", "m2"],
      ["Effort", "medium", "high"],
      ["Skill agent/new", "not installed", "4".repeat(64)],
      ["Skill agent/notes", "2".repeat(64), "9".repeat(64)],
      ["Granted rule r2", "granted", "not granted"],
      ["Granted rule r3", "not granted", "granted"],
    ]);
  });

  it("reads a field an older run did not have as not set", () => {
    const old: Record<string, unknown> = { ...base };
    delete old.model_roles;
    delete old.subagent_model;
    const d = diffFingerprints(old, base);
    expect(d.map((c) => c.field)).toEqual(["Subagent model"]);
    expect(d[0]).toMatchObject({ before: "not set", after: "inherit" });
  });
});

describe("runs", () => {
  it("starts a run and shows the one-active-run refusal inline, not only as a toast", async () => {
    const calls = mockFetch({
      "GET /v1/agents/a1/eval-runs": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/agents/a1/eval-questions": () => json(200, { questions: [question(1)] }),
      "POST /v1/agents/a1/eval-runs": () => json(409, { error: { code: "eval_run_active", message: "busy" } }),
    });
    wrap(<RunsSection agentId={A} frozen={false} onOpen={() => {}} />);
    const start = await screen.findByRole("button", { name: "Start run" });
    await waitFor(() => expect(start).not.toBeDisabled());
    fireEvent.click(start);
    expect(await screen.findByText(/A run is already in progress for this agent/)).toBeInTheDocument();
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
  });

  it("cancels an active run after a confirmation", async () => {
    const calls = mockFetch({
      "GET /v1/agents/a1/eval-runs": () => json(200, { items: [run("r1", { status: "running", pass_rate: null, counts: { total: 4, passed: 1, failed: 0, errored: 0, not_scored: 3, refused_writes: 0 } })], next_cursor: null }),
      "GET /v1/agents/a1/eval-questions": () => json(200, { questions: [question(1)] }),
      "POST /v1/eval-runs/r1/cancel": () => json(202, run("r1", { status: "running" })),
    });
    wrap(<RunsSection agentId={A} frozen={false} onOpen={() => {}} />);
    expect(await screen.findByRole("button", { name: "Start run" })).toBeDisabled();
    expect(screen.getByText(/A run is in progress/)).toBeInTheDocument();
    fireEvent.click((await screen.findAllByRole("button", { name: "Cancel run" }))[0]!);
    fireEvent.click(await screen.findByRole("button", { name: "Stop the run" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/eval-runs/r1/cancel")).toBe(true));
  });

  it("shows the regression notice with counts only", async () => {
    mockFetch({
      "GET /v1/agents/a1/eval-runs": () => json(200, { items: [run("r2", { regression: { from_run_id: "r1", drop_points: 12.5 } })], next_cursor: null }),
      "GET /v1/agents/a1/eval-questions": () => json(200, { questions: [question(1)] }),
    });
    wrap(<RunsSection agentId={A} frozen={false} onOpen={() => {}} />);
    const note = await screen.findByTestId("regression-notice");
    expect(note).toHaveTextContent("fell by 12.5 points");
  });

  it("a run's page shows results, the reason in words and only the changed configuration fields", async () => {
    const fp = (model: string) => ({ instructions_sha256: "1".repeat(64), harness: "h", model: { provider: "p", endpoint_id: "e", model_id: model }, effort: null });
    const detail = (id: string, model: string, over: Partial<EvalRun> = {}) => ({
      ...run(id, over),
      fingerprint: fp(model),
      next_results_after: null,
      results: [
        { position: 1, question: { id: "q1", text: "How many?" }, expected_kind: "numeric", expected: { number: 4, tolerance: 0, text: null, criteria: null }, outcome: "fail", reason_code: "no_answer_line", extracted_value: null, answer: "About four", judge: null, refused_writes: 2, conversation_id: null, started_at: null, finished_at: null },
      ],
    });
    mockFetch({
      "GET /v1/eval-runs/r2": () => json(200, detail("r2", "m2")),
      "GET /v1/eval-runs/r1": () => json(200, detail("r1", "m1", { created_at: "2026-11-09T10:00:00Z" })),
      "GET /v1/agents/a1/eval-runs": () => json(200, { items: [run("r2"), run("r1", { created_at: "2026-11-09T10:00:00Z" })], next_cursor: null }),
    });
    wrap(<RunDetail agentId={A} runId="r2" canOwn={false} onBack={() => {}} />);
    expect((await screen.findAllByText("The reply did not end with an ANSWER: line.")).length).toBeGreaterThan(0);
    // The changed field only: the model, earlier then later.
    const table = await screen.findByRole("region", { name: "Changed configuration fields" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(2); // header + one change
    expect(within(rows[1]!).getByText("Model")).toBeInTheDocument();
    expect(within(rows[1]!).getByText("m1")).toBeInTheDocument();
    expect(within(rows[1]!).getByText("m2")).toBeInTheDocument();
    // Delete is for owners.
    expect(screen.queryByRole("button", { name: "Delete run" })).not.toBeInTheDocument();
  });
});

describe("page availability", () => {
  it("has no Evals page for a BYO agent", () => {
    expect(isPageAvailable("evals", { byo: true, previews: true, terminal: true })).toBe(false);
    expect(isPageAvailable("evals", { byo: false, previews: false, terminal: false })).toBe(true);
  });
});

describe("cron in words", () => {
  it("describes the common shapes and warns about the hourly minimum", () => {
    expect(describeCron("0 6 * * *", "UTC")).toBe("Every day at 06:00 (UTC).");
    expect(describeCron("30 9 * * 1-5", "Europe/Zurich")).toBe("Every Monday to Friday at 09:30 (Europe/Zurich).");
    expect(describeCron("0 8 * * 1", "UTC")).toBe("Every Monday at 08:00 (UTC).");
    expect(describeCron("15 */6 * * *", "UTC")).toBe("Every 6 hours, at 15 minutes past (UTC).");
    expect(describeCron("0 8 5 * *", "UTC")).toBe("On day 5 of every month at 08:00 (UTC).");
    expect(describeCron("nonsense", "UTC")).toBeNull();
    expect(describeCron("0 8 * * 9-1", "UTC")).toBeNull();
    expect(firesMoreThanHourly("*/10 * * * *")).toBe(true);
    expect(firesMoreThanHourly("0,30 9 * * *")).toBe(true);
    expect(firesMoreThanHourly("0 * * * *")).toBe(false);
  });
});
