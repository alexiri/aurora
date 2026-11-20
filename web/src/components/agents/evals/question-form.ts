import type { CreateEvalQuestion, EvalQuestion, PatchEvalQuestion } from "@/lib/api/evals";
import { EVAL_CRITERIA_CHARS, EVAL_EXPECTED_TEXT_CHARS, EVAL_QUESTION_CHARS } from "@/lib/api/evals";

export type QuestionKind = "numeric" | "text" | "judged";

/** What the question dialog holds: every field as typed. */
export interface QuestionDraft {
  question: string;
  kind: QuestionKind;
  number: string;
  tolerance: string;
  text: string;
  criteria: string;
  /** Included in runs. */
  enabled: boolean;
}

export type QuestionErrors = Partial<Record<"question" | "number" | "tolerance" | "text" | "criteria", string>>;

export const emptyDraft: QuestionDraft = { question: "", kind: "numeric", number: "", tolerance: "", text: "", criteria: "", enabled: true };

export const draftOf = (q: EvalQuestion): QuestionDraft => ({
  question: q.question,
  kind: q.expected_kind as QuestionKind,
  number: q.expected_number == null ? "" : String(q.expected_number),
  tolerance: q.tolerance == null || q.tolerance === 0 ? "" : String(q.tolerance),
  text: q.expected_text ?? "",
  criteria: q.criteria ?? "",
  enabled: q.enabled,
});

/** Characters the way the API counts them (Unicode scalar values). */
export const charCount = (s: string) => Array.from(s).length;

/** A plain decimal number: an optional sign, digits, an optional fraction. No exponent, no thousands separators. */
export function parseNumber(s: string): number | null {
  const t = s.trim();
  if (!/^[+-]?(\d+\.?\d*|\.\d+)$/.test(t)) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/** The same rules the API applies (it stays authoritative): what stops a form from being sent. */
export function validateQuestion(d: QuestionDraft): QuestionErrors {
  const e: QuestionErrors = {};
  if (d.question.trim() === "") e.question = "Write the question the agent will be asked.";
  else if (charCount(d.question) > EVAL_QUESTION_CHARS) e.question = `Keep the question under ${EVAL_QUESTION_CHARS} characters.`;
  if (d.kind === "numeric") {
    if (parseNumber(d.number) === null) e.number = "Enter the expected answer as a number, for example 42 or 3.5.";
    if (d.tolerance.trim() !== "") {
      const t = parseNumber(d.tolerance);
      if (t === null || t < 0) e.tolerance = "Enter the tolerance as a number that is zero or more, or leave it empty for an exact answer.";
    }
  } else if (d.kind === "text") {
    if (d.text.trim() === "") e.text = "Enter the expected answer.";
    else if (charCount(d.text) > EVAL_EXPECTED_TEXT_CHARS) e.text = `Keep the expected answer under ${EVAL_EXPECTED_TEXT_CHARS} characters.`;
  } else {
    if (d.criteria.trim() === "") e.criteria = "Describe what a good answer contains. The grader reads this.";
    else if (charCount(d.criteria) > EVAL_CRITERIA_CHARS) e.criteria = `Keep the criteria under ${EVAL_CRITERIA_CHARS} characters.`;
  }
  return e;
}

export function toCreateBody(d: QuestionDraft): CreateEvalQuestion {
  const base = { question: d.question.trim(), expected_kind: d.kind, enabled: d.enabled };
  if (d.kind === "numeric") {
    return { ...base, expected_number: parseNumber(d.number)!, tolerance: d.tolerance.trim() === "" ? 0 : parseNumber(d.tolerance)! };
  }
  if (d.kind === "text") return { ...base, expected_text: d.text.trim() };
  return { ...base, criteria: d.criteria.trim() };
}

/** A patch holds only what changed: the kind never changes. */
export function toPatchBody(d: QuestionDraft, before: EvalQuestion): PatchEvalQuestion {
  const next = toCreateBody(d);
  const out: PatchEvalQuestion = {};
  if (next.question !== before.question) out.question = next.question;
  if (d.enabled !== before.enabled) out.enabled = d.enabled;
  if (d.kind === "numeric") {
    if (next.expected_number !== before.expected_number) out.expected_number = next.expected_number;
    if ((next.tolerance ?? 0) !== (before.tolerance ?? 0)) out.tolerance = next.tolerance;
  } else if (d.kind === "text") {
    if (next.expected_text !== before.expected_text) out.expected_text = next.expected_text;
  } else if (next.criteria !== before.criteria) out.criteria = next.criteria;
  return out;
}

/** One line for the questions list: what the agent's answer is compared with. */
export function expectedSummary(q: EvalQuestion): string {
  if (q.expected_kind === "numeric") {
    const tol = q.tolerance ? ` ± ${q.tolerance}` : "";
    return `${q.expected_number ?? "?"}${tol}`;
  }
  if (q.expected_kind === "text") return q.expected_text ?? "";
  return q.criteria ?? "";
}

export const KIND_LABEL: Record<string, string> = { numeric: "Number", text: "Text", judged: "Graded" };
