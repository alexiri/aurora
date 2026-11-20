"use client";

import { useState } from "react";

import { Notice } from "@/components/ui/notice";
import { useEvalQuestions } from "@/lib/api/evals";

import { WRITES_HELP } from "./copy";
import { JudgeSection } from "./judge-section";
import { QuestionsSection } from "./questions-section";
import { RunDetail } from "./run-detail";
import { RunsSection } from "./runs-section";
import { SchedulesSection } from "./schedules-section";

/**
 * The Evals page of the Behaviour section (`?tab=evals`, D-289(e), docs/14 §Agent page): Questions, Judge, Runs and
 * Schedules for a hosted agent, and a run's own page (results and the configuration difference). Expected answers and
 * answers live only in the query cache while the page is open (SEC-116); the open run is page state, not part of the URL.
 */
export function EvalsTab({
  agentId,
  frozen,
  canOwn,
  defaultTimezone,
}: {
  agentId: string;
  frozen: boolean;
  canOwn: boolean;
  defaultTimezone?: string;
}) {
  const [open, setOpen] = useState<{ id: string; compareWith?: string } | null>(null);
  const questions = useEvalQuestions(agentId);
  // (P5-POLISH-6, C6) A fresh agent shows one empty state (the Questions section's); Judge, Runs and Schedules appear after the first question.
  const hasQuestions = (questions.data?.length ?? 0) > 0;
  if (open) {
    return <RunDetail key={open.id} agentId={agentId} runId={open.id} compareWith={open.compareWith} canOwn={canOwn} onBack={() => setOpen(null)} onOpenRun={(id) => setOpen({ id })} />;
  }
  return (
    <div className="space-y-8">
      {hasQuestions ? <Notice tone="neutral">{WRITES_HELP}</Notice> : null}
      <QuestionsSection agentId={agentId} />
      {hasQuestions ? (
        <>
          <JudgeSection agentId={agentId} canOwn={canOwn} />
          <RunsSection agentId={agentId} frozen={frozen} onOpen={(id, compareWith) => setOpen({ id, compareWith })} />
          <SchedulesSection agentId={agentId} defaultTimezone={defaultTimezone} canOwn={canOwn} onRunStarted={(id) => setOpen({ id })} />
        </>
      ) : null}
    </div>
  );
}
