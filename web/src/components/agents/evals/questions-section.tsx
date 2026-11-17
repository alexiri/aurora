"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { EVAL_QUESTIONS_PER_AGENT, useDeleteEvalQuestion, useEvalQuestions, type EvalQuestion } from "@/lib/api/evals";

import { evalErrorMessage } from "./copy";
import { KIND_LABEL, expectedSummary } from "./question-form";
import { QuestionDialog } from "./question-dialog";

/**
 * Questions (docs/14 §Agent page, Evals). A table from `md` up and a stacked list on a phone, with the same fields.
 * The 100-question cap is explained in the header and on the disabled button, never hidden.
 */
export function QuestionsSection({ agentId }: { agentId: string }) {
  const q = useEvalQuestions(agentId);
  const del = useDeleteEvalQuestion(agentId);
  const [dialog, setDialog] = useState<{ question?: EvalQuestion } | null>(null);
  const [removing, setRemoving] = useState<EvalQuestion | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const items = q.data;
  const full = items.length >= EVAL_QUESTIONS_PER_AGENT;
  const enabled = items.filter((x) => x.enabled).length;

  const actions = (x: EvalQuestion) => (
    <>
      <Button size="sm" variant="outline" aria-label={`Edit question ${x.position}`} onClick={() => setDialog({ question: x })}>
        Edit
      </Button>
      <Button size="sm" variant="outline" aria-label={`Delete question ${x.position}`} onClick={() => setRemoving(x)}>
        Delete
      </Button>
    </>
  );

  return (
    <Section
      id="eval-questions"
      title="Questions"
      description={`${items.length} of ${EVAL_QUESTIONS_PER_AGENT} questions, ${enabled} included in runs. Questions run one at a time, in this order.`}
      actions={
        <Button size="sm" disabled={full} aria-describedby={full ? "evq-cap" : undefined} onClick={() => setDialog({})}>
          Add a question
        </Button>
      }
    >
      {full ? (
        <Notice tone="pending" role="status" id="evq-cap">
          This agent has {EVAL_QUESTIONS_PER_AGENT} questions, the most it can have. Delete one to add another.
        </Notice>
      ) : null}
      {del.error ? (
        <Notice tone="danger" role="alert">
          {evalErrorMessage(del.error)}
        </Notice>
      ) : null}
      {items.length === 0 ? (
        <EmptyState
          title="No questions yet"
          action={
            <Button size="sm" onClick={() => setDialog({})}>
              Add a question
            </Button>
          }
        >
          A question is something you already know the answer to. A run asks the agent every question and scores the replies, so you can tell when a change makes it better or worse.
        </EmptyState>
      ) : (
        <>
          <div className="hidden md:block">
            <DataTable caption="Eval questions" className="min-w-0">
              <thead>
                <tr>
                  <Th>#</Th>
                  <Th>Question</Th>
                  <Th>Checked as</Th>
                  <Th>Expected</Th>
                  <Th>
                    <span className="sr-only">Actions</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {items.map((x) => (
                  <tr key={x.id}>
                    <Td className="tabular-nums text-ink-2">{x.position}</Td>
                    <Td className="max-w-md whitespace-normal break-words">
                      {x.question}
                      {x.enabled ? null : <Badge className="ml-2">Off</Badge>}
                    </Td>
                    <Td>{KIND_LABEL[x.expected_kind] ?? x.expected_kind}</Td>
                    <Td className="max-w-xs whitespace-normal break-words text-ink-2">{expectedSummary(x)}</Td>
                    <Td className="text-right">
                      <div className="flex justify-end gap-2">{actions(x)}</div>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </div>
          <ItemList aria-label="Eval questions" className="md:hidden">
            {items.map((x) => (
              <ListRow key={x.id} actions={actions(x)}>
                <div className="min-w-0 space-y-1 text-sm">
                  <p className="break-words font-medium text-ink">
                    {x.position}. {x.question}
                  </p>
                  <p className="text-xs text-muted">
                    {KIND_LABEL[x.expected_kind] ?? x.expected_kind}
                    {x.enabled ? "" : " · off"}
                  </p>
                  <p className="break-words text-xs text-ink-2">{expectedSummary(x)}</p>
                </div>
              </ListRow>
            ))}
          </ItemList>
        </>
      )}
      {dialog ? (
        <QuestionDialog
          key={dialog.question?.id ?? "new"}
          agentId={agentId}
          open
          onOpenChange={(o) => {
            if (!o) setDialog(null);
          }}
          question={dialog.question}
          count={items.length}
        />
      ) : null}
      <ConfirmDialog
        open={!!removing}
        onOpenChange={(o) => {
          if (!o) setRemoving(null);
        }}
        title="Delete this question?"
        description={removing ? `Question ${removing.position} is removed from the list. Results of past runs keep their copy of it.` : undefined}
        confirmLabel="Delete question"
        danger
        busy={del.isPending}
        onConfirm={() => {
          if (removing) del.mutate(removing.id, { onSettled: () => setRemoving(null) });
        }}
      />
    </Section>
  );
}
