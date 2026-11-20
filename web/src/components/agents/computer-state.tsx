"use client";

import { useEffect, useState } from "react";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import { useRetrySandbox, useRestartSandbox, useSandbox, useSleepSandbox, useWakeSandbox } from "@/lib/api/conversations";
import type { ComputerView, SandboxView } from "@/lib/api/conversations";
import { errorCopy } from "@/lib/chat/transcript";
import { COMPUTER_WORD, label } from "@/lib/labels";
import { durationWords } from "@/lib/settings-words";
import { useUndoToast } from "@/components/ui/undo-toast";

/** States where Restart has nothing to do (the API answers 409 `restart_not_applicable`, C-626). */
const RESTART_NOT_APPLICABLE = new Set(["stopping", "stopped", "unprovisioned"]);

/**
 * (P5-REFINE-15, D-412) The computer in words. The API's `computer` (C-1059) when it is there; otherwise derived from
 * `state` (`running`→on, `starting`→starting, `stopping`→stopping, `failed`→failed, `stopped`/`unprovisioned`→off).
 */
export function computerOf(v: SandboxView): Pick<ComputerView, "word" | "sleeps_in_secs" | "phase" | "started_at"> {
  if (v.computer) return v.computer;
  const word = v.state === "running" ? "on" : v.state === "starting" ? "starting" : v.state === "stopping" ? "stopping" : v.state === "failed" ? "failed" : "off";
  return { word, sleeps_in_secs: null, phase: null, started_at: v.started_at };
}

/** "20 s" under a minute and a half, else in minutes ("5 minutes"). */
function shortWords(secs: number): string {
  return secs < 90 ? `${Math.max(0, Math.round(secs))} s` : durationWords(Math.round(secs / 60) * 60);
}

/** The seconds left, counting down locally between the 5 s polls. */
function useCountdown(secs: number | null, stamp: number): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (secs === null) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [secs, stamp]);
  return secs === null ? null : Math.max(0, secs - Math.max(0, Math.floor((now - stamp) / 1000)));
}

const TONE: Record<ComputerView["word"], StatusTone> = { on: "ok", off: "idle", starting: "idle", stopping: "idle", failed: "fail" };

/**
 * (P5-REFINE-15, D-408, D-412) One line of state above every Computer sub-page, the phase in words and what follows,
 * with the actions that fit it: **Wake it now**, **Sleep now** and **Restart** from Set up (`computer.actions`),
 * **Try again** on a failed start (always). Files and the Terminal connect by themselves, so none of them is needed to use them.
 */
export function ComputerStateLine({ agentId, agentName, frozen = false }: { agentId: string; agentName?: string; frozen?: boolean }) {
  const q = useSandbox(agentId);
  const restart = useRestartSandbox(agentId);
  const retry = useRetrySandbox(agentId);
  const wake = useWakeSandbox(agentId);
  const sleep = useSleepSandbox(agentId);
  const toast = useUndoToast();
  const [open, setOpen] = useState(false);
  // (P5-APIGAP-1) After a 202 the button stays pending until the status says starting or on; a start that never shows
  // (the runner is busy) frees the button again after a minute.
  useEffect(() => {
    if (!wake.isSuccess) return;
    const t = setTimeout(() => wake.reset(), 60_000);
    return () => clearTimeout(t);
  }, [wake.isSuccess, wake]);
  const v = q.data;
  const c = v ? computerOf(v) : null;
  const countdown = useCountdown(c?.sleeps_in_secs ?? null, q.dataUpdatedAt);
  if (!v || !c) return null;
  const name = agentName ?? "The agent";
  const word = c.word;
  const asleep = word === "off" || word === "stopping";
  const wakePending = wake.isPending || (wake.isSuccess && asleep);
  // (FIX-049, C-626) Restart is disabled where it would answer 409, and on a failed computer of a frozen agent (423).
  const restartBlocked = RESTART_NOT_APPLICABLE.has(v.state) || (v.state === "failed" && frozen);
  // (C-1046) Idle stop in words; nothing when it is off.
  const after = v.idle_stop_after_secs ? `sleeps after ${durationWords(v.idle_stop_after_secs)} of quiet` : null;
  const idleNow = word === "on" && countdown !== null;
  const reason = word === "failed" ? `${errorCopy(v.failure_code).message} ${v.failure_message ?? ""}`.trim() : null;
  return (
    <section aria-label="Computer state" className="space-y-2" data-testid="computer-state-line">
      <StatusLine
        tone={TONE[word]}
        className="text-base"
        data-testid="computer-state"
        action={
          <span className="flex flex-wrap items-center gap-2">
            {word === "failed" && !frozen ? (
              <Button size="sm" disabled={retry.isPending || !v.retryable} onClick={() => retry.mutate(undefined)}>
                {retry.isPending || !v.retryable ? "Start in flight…" : "Try again"}
              </Button>
            ) : null}
            <Altitude id="computer.actions" level="setup">
              {asleep && word === "off" && !frozen ? (
                <Button size="sm" disabled={wakePending} onClick={() => wake.mutate(undefined)}>
                  {wakePending ? "Waking…" : "Wake it now"}
                </Button>
              ) : null}
              {word === "on" && !frozen ? (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={sleep.isPending}
                  onClick={() => sleep.mutate(undefined, { onSuccess: () => toast(`${name}'s computer is going to sleep`) })}
                >
                  Sleep now
                </Button>
              ) : null}
              {word === "on" ? (
                <Button variant="outline" size="sm" disabled={restart.isPending || restartBlocked} onClick={() => setOpen(true)}>
                  Restart
                </Button>
              ) : null}
            </Altitude>
          </span>
        }
      >
        <span data-testid="computer-state-words">
        {word === "on" ? (
          <>
            {name}&apos;s computer is <strong className="font-semibold text-ink">{label(COMPUTER_WORD, word).toLowerCase()}</strong>
            {idleNow ? ` · sleeps in ${shortWords(countdown)} when idle` : after ? ` · ${after}` : ""}
          </>
        ) : word === "off" ? (
          <>
            {name}&apos;s computer is <strong className="font-semibold text-ink">{label(COMPUTER_WORD, word).toLowerCase()}</strong> · it starts when {name} needs it
          </>
        ) : word === "starting" ? (
          <>
            <span aria-hidden="true" className="mr-1.5 inline-block size-3.5 animate-spin rounded-full border-2 border-line-strong border-t-accent align-middle motion-reduce:animate-none" />
            <strong className="font-semibold text-ink">{label(COMPUTER_WORD, word)}</strong> {name}&apos;s computer… <span className="text-muted">Usually under 20 seconds.</span>
          </>
        ) : word === "stopping" ? (
          <>
            <strong className="font-semibold text-ink">{label(COMPUTER_WORD, word)}</strong>…
          </>
        ) : (
          <>
            {name}&apos;s computer <strong className="font-semibold">couldn&apos;t start</strong>
            {reason ? `: ${reason}` : ""}
          </>
        )}
        </span>
      </StatusLine>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title="Restart this computer?"
        description="Running processes will be stopped and the computer boots again."
        confirmLabel="Restart"
        onConfirm={() => {
          restart.mutate(undefined, { onSettled: () => setOpen(false) });
        }}
      />
      <ErrorNotice error={restart.error} />
      <ErrorNotice error={wake.error} />
      <ErrorNotice error={sleep.error} />
      {word === "failed" && retry.error ? <ErrorNotice error={retry.error} /> : null}
      {v.restart_to_update ? <Notice role="status">Configuration changed. Restart to update this computer.</Notice> : null}
    </section>
  );
}
