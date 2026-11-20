import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";

type Tone = "neutral" | "accent" | "danger";
type BadgeProps = { tone: Tone };

/** Outcome of one question (C-867) in words and a tone. Colour is never the only signal: the badge carries the word. */
export const OUTCOME: Record<string, { label: string; tone: BadgeProps["tone"] }> = {
  pass: { label: "Passed", tone: "accent" },
  fail: { label: "Failed", tone: "danger" },
  error: { label: "Error", tone: "danger" },
  not_scored: { label: "Not scored", tone: "neutral" },
};

export const RUN_STATUS: Record<string, { label: string; tone: BadgeProps["tone"] }> = {
  queued: { label: "Queued", tone: "neutral" },
  running: { label: "Running", tone: "accent" },
  done: { label: "Done", tone: "accent" },
  failed: { label: "Failed", tone: "danger" },
  cancelled: { label: "Cancelled", tone: "neutral" },
};

/** `reason_code` vocabulary of C-867, in words. A failed turn's own `error_code` falls through to a readable form. */
export const REASON: Record<string, string> = {
  no_answer_line: "The reply did not end with an ANSWER: line.",
  answer_not_a_number: "The ANSWER: line is not a number.",
  judge_not_available: "Judged questions cannot be scored yet.",
  timeout: "The agent did not finish in time, so the turn was stopped.",
  cancelled: "The run was cancelled before this question.",
  access_lost: "The person who started the run lost access.",
  frozen: "The agent was paused.",
  judge_not_configured: "No judge key is set. Choose one in the judge settings.",
  judge_not_granted: "The judge key is no longer granted to this agent.",
  judge_not_permitted: "Your organization does not permit the judge key's scope.",
  judge_not_usable: "The judge key or its model cannot be used right now.",
  judge_unavailable: "The judge's model provider could not be reached.",
  judge_timeout: "The judge took too long to answer.",
  judge_unparseable: "The judge's answer could not be read.",
};

export const reasonText = (code: string | null | undefined): string => {
  if (!code) return "";
  return REASON[code] ?? `The turn failed (${code.replace(/_/g, " ")}).`;
};

/** Run-level `error_code` (a cancelled or failed run) in words. */
export const RUN_REASON: Record<string, string> = {
  access_lost: "Cancelled: the person who started the run lost access to the agent.",
  frozen: "Cancelled: the agent was paused.",
  cancelled: "Cancelled by a person.",
};

const EVAL_MESSAGES: Record<string, string> = {
  eval_not_supported: "Tests are available for hosted agents only.",
  eval_question_limit: "This agent has the most test questions it can have (100). Delete one to add another.",
  eval_no_questions: "Add at least one enabled question before starting a run.",
  eval_run_active: "A run is already in progress for this agent. Wait for it to finish or cancel it.",
  eval_run_finished: "This run has already finished.",
  eval_runs_limited: "This agent has reached its test run limit for today (20 runs a day, including scheduled ones). Try again tomorrow, UTC.",
  eval_schedule_limit: "This agent has the most test schedules it can have (5).",
  eval_judge_not_granted: "The judge key must be an AI key that is granted to this agent. Grant it first, then choose it here.",
  eval_judge_not_usable: "That key cannot be used right now. Check its status under the agent's model settings.",
  eval_judge_not_permitted: "Your organization's governance does not permit this key or its scope for this use.",
  frozen: "This agent is paused, so no run can start.",
};

/** A message for an eval refusal (C-872), otherwise the app-wide wording. */
export function evalErrorMessage(err: unknown): string {
  if (err instanceof ApiError && EVAL_MESSAGES[err.code]) return EVAL_MESSAGES[err.code]!;
  return errorMessage(err);
}

export const isCode = (err: unknown, ...codes: string[]) => err instanceof ApiError && codes.includes(err.code);

/** The `ANSWER:` help: what the agent is told. */
export const ANSWER_HELP =
  "For number and text questions the agent is asked to finish its reply with a last line, ANSWER: followed by its answer. That line is compared with the expected answer; text is compared ignoring case and extra spaces.";

export const WRITES_HELP =
  "A run is a real conversation, so the agent works as it normally does, but every action that would change something outside is refused and the agent is told so: sending messages, writing memory or skills, creating schedules, and any request that is not a read. Refused actions are counted per question.";
