"use client";

import { useState, type FormEvent } from "react";

import { Field, inputClass } from "@/components/ui/field";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import { fieldErrors } from "@/lib/api/errors";
import {
  EVAL_CRITERIA_CHARS,
  EVAL_EXPECTED_TEXT_CHARS,
  EVAL_QUESTION_CHARS,
  useCreateEvalQuestion,
  usePatchEvalQuestion,
  type EvalQuestion,
} from "@/lib/api/evals";

import { ANSWER_HELP, evalErrorMessage } from "./copy";
import {
  KIND_LABEL,
  charCount,
  draftOf,
  emptyDraft,
  toCreateBody,
  toPatchBody,
  validateQuestion,
  type QuestionDraft,
  type QuestionKind,
} from "./question-form";

const KINDS: QuestionKind[] = ["numeric", "text", "judged"];

/**
 * Add or edit one question (C-861). The kind is fixed once a question exists (the API has no way to change it).
 * The dialog validates what it can (the API stays authoritative) and shows the API's own field messages too.
 */
export function QuestionDialog({
  agentId,
  open,
  onOpenChange,
  question,
  count,
}: {
  agentId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The question being edited; absent for a new one. */
  question?: EvalQuestion;
  /** Questions the agent has now (the position field's upper bound). */
  count: number;
}) {
  const create = useCreateEvalQuestion(agentId);
  const patch = usePatchEvalQuestion(agentId);
  const [draft, setDraft] = useState<QuestionDraft>(() => (question ? draftOf(question) : emptyDraft));
  const [position, setPosition] = useState(question ? String(question.position) : "");
  const [tried, setTried] = useState(false);
  const mut = question ? patch : create;
  const server = fieldErrors(mut.error);
  const local = tried ? validateQuestion(draft) : {};
  const set = <K extends keyof QuestionDraft>(k: K, v: QuestionDraft[K]) => setDraft((d) => ({ ...d, [k]: v }));
  const err = {
    question: local.question ?? server.question,
    number: local.number ?? server.expected_number,
    tolerance: local.tolerance ?? server.tolerance,
    text: local.text ?? server.expected_text,
    criteria: local.criteria ?? server.criteria,
  };

  function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (Object.keys(validateQuestion(draft)).length > 0) return;
    const done = () => onOpenChange(false);
    if (question) {
      const body = toPatchBody(draft, question);
      const pos = Number.parseInt(position, 10);
      if (Number.isInteger(pos) && pos !== question.position && pos >= 1) body.position = Math.min(pos, count);
      if (Object.keys(body).length === 0) return done();
      patch.mutate({ id: question.id, body }, { onSuccess: done });
    } else {
      create.mutate(toCreateBody(draft), { onSuccess: done });
    }
  }

  const unexplained = mut.error && Object.keys(server).length === 0;
  return (
    <FormDialog
      open={open}
      onOpenChange={onOpenChange}
      title={question ? "Edit question" : "Add a question"}
      description="The agent is asked the question in a fresh conversation. Its reply is compared with the expected answer."
      submitLabel={question ? "Save question" : "Add question"}
      busy={mut.isPending}
      size="lg"
      onSubmit={submit}
    >
      <Field id="evq-question" label="Question" error={err.question} hint={`${charCount(draft.question)} of ${EVAL_QUESTION_CHARS} characters`}>
        <textarea id="evq-question" className={inputClass} rows={4} value={draft.question} onChange={(e) => set("question", e.target.value)} />
      </Field>

      <div className="space-y-1.5">
        <Segmented
          label="How the answer is checked"
          value={draft.kind}
          disabled={!!question}
          options={KINDS.map((k) => ({ value: k, label: KIND_LABEL[k]! }))}
          onChange={(k) => set("kind", k)}
        />
        <p className="text-xs text-muted">
          {question
            ? "The kind cannot be changed. Delete the question and add it again to change it."
            : "Number and text answers are checked exactly. Judged answers are read by the judge model against your criteria."}
        </p>
      </div>

      {draft.kind === "numeric" ? (
        <div className="grid gap-4 sm:grid-cols-2">
          <Field id="evq-number" label="Expected number" error={err.number}>
            <input id="evq-number" className={inputClass} inputMode="decimal" value={draft.number} onChange={(e) => set("number", e.target.value)} />
          </Field>
          <Field id="evq-tolerance" label="Tolerance (optional)" error={err.tolerance} hint="How far the answer may be from the expected number. Empty means exact.">
            <input id="evq-tolerance" className={inputClass} inputMode="decimal" value={draft.tolerance} onChange={(e) => set("tolerance", e.target.value)} />
          </Field>
        </div>
      ) : null}
      {draft.kind === "text" ? (
        <Field id="evq-text" label="Expected answer" error={err.text} hint={`${charCount(draft.text)} of ${EVAL_EXPECTED_TEXT_CHARS} characters. Compared ignoring case and extra spaces.`}>
          <input id="evq-text" className={inputClass} value={draft.text} onChange={(e) => set("text", e.target.value)} />
        </Field>
      ) : null}
      {draft.kind === "judged" ? (
        <Field id="evq-criteria" label="Criteria" error={err.criteria} hint={`${charCount(draft.criteria)} of ${EVAL_CRITERIA_CHARS} characters. What a good answer contains; the judge sees this and the answer.`}>
          <textarea id="evq-criteria" className={inputClass} rows={3} value={draft.criteria} onChange={(e) => set("criteria", e.target.value)} />
        </Field>
      ) : null}
      {draft.kind !== "judged" ? <p className="text-xs text-muted">{ANSWER_HELP}</p> : null}

      {question ? (
        <Field id="evq-position" label="Position" hint={`Where the question runs, 1 to ${count}. The others move to make room.`}>
          <input id="evq-position" className={`${inputClass} sm:w-28`} inputMode="numeric" value={position} onChange={(e) => setPosition(e.target.value)} />
        </Field>
      ) : null}
      <CheckboxField id="evq-enabled" label="Include in runs" hint="A question that is off stays in the list but is skipped." checked={draft.enabled} onChange={(v) => set("enabled", v)} />

      {unexplained ? (
        <Notice tone="danger" role="alert">
          {evalErrorMessage(mut.error)}
        </Notice>
      ) : null}
    </FormDialog>
  );
}
