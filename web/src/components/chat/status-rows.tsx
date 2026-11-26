"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useState } from "react";

import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { chatApi, useRetrySandbox, useSandbox, type Followup } from "@/lib/api/conversations";
import { errorCopy, type ErrorLink } from "@/lib/chat/transcript";
import { wsRoutes } from "@/lib/routes";

import type { AgentIdent } from "./types";
/** While a turn runs: the agent avatar and a `subtle` bubble with three dots and the activity line. */
export function TypingRow({ agent, line }: { agent?: AgentIdent; line?: string }) {
  return (
    <div role="status" className="flex items-end gap-2">
      <Avatar name={agent?.name ?? "Agent"} url={agent?.avatarUrl} tintKey={agent?.id} size={32} className="shrink-0" />
      <div className="flex max-w-chat-card items-center gap-2 rounded-card rounded-tl-sm bg-subtle px-3.5 py-2.5 text-sm text-ink-2">
        <span aria-hidden="true" className="flex gap-1">
          <span className="size-1.5 animate-pulse rounded-full bg-muted motion-reduce:animate-none" />
          <span className="size-1.5 animate-pulse rounded-full bg-muted [animation-delay:150ms] motion-reduce:animate-none" />
          <span className="size-1.5 animate-pulse rounded-full bg-muted [animation-delay:300ms] motion-reduce:animate-none" />
        </span>
        <span className="min-w-0 break-words">{line || "Working…"}</span>
      </div>
    </div>
  );
}

/** Redacted event (RFE-010): a dashed, muted row. The content was replaced in place; only the marker remains. */
export function RedactedRow({ by, reason, date }: { by?: string; reason?: string; date: string }) {
  return (
    <p className="mx-auto w-fit max-w-full rounded-control border border-dashed border-line-strong px-3 py-1.5 text-center text-xs italic text-muted">
      Message redacted{by ? ` by ${by}` : ""}
      {reason ? ` (${reason})` : ""}
      {date ? ` · ${date}` : ""}
    </p>
  );
}

/** A `danger` block in the agent column with the error copy and the deep link that fixes it. */
export function ErrorBlock({
  code,
  message,
  detail,
  base,
  agentId,
  canOwn = false,
  children,
}: {
  code?: string | null;
  message?: string;
  /** (P5-REFINE-7, C-1076) The harness's technical line from the `error` event: always under Details, never in the sentence. */
  detail?: string;
  base: string;
  /** The failing agent (id or handle): "Choose a model" and "Agent access" open **its** page, whatever page the chat is on (FIX-170). */
  agentId?: string;
  /** Workspace owner: gets the "Agent access" link. */
  canOwn?: boolean;
  /** Reserved (FIX-029, C-615): the retry button is passed as `children` by the chat's error blocks. */
  children?: React.ReactNode;
}) {
  const copy = errorCopy(code, message);
  const [details, setDetails] = useState(false);
  // `detail` (C-1076) is the technical line; an older event has only `message`, shown when the code has no copy of its own.
  const technical = detail || (copy.showServerMessage ? message : "");
  // The chat lives on the agent page, so its tabs are addressed relative to the current path (`?tab=`, C-506).
  const pathname = usePathname() ?? "";
  // `base` is the workspace's own `/o/<org>/w/<ws>`: the route helper builds the agent's page from its parts.
  const parts = /^\/o\/([^/]+)\/w\/([^/]+)/.exec(base);
  const routes = parts ? wsRoutes(decodeURIComponent(parts[1]!), decodeURIComponent(parts[2]!)) : null;
  const hrefOf = (l: ErrorLink): string | undefined => {
    if (l.path === "credentials") {
      // The gateway's `credentials` link names the old page; its tab decides the Apps & accounts tab (C-960).
      return `${base}/apps?tab=${l.tab === "apps" || l.tab === "oauth" ? "apps" : "accounts"}${l.tab === "llm" ? "&kind=ai-key" : ""}`;
    }
    if (l.path === "access" || l.path === "instructions") {
      // (FIX-170) The failing agent's own page; "Choose a model" lands on its Default model field.
      if (agentId && routes) return `${routes.agent(agentId, { tab: l.path })}${l.path === "instructions" ? "#a-model" : ""}`;
      return `${pathname}?tab=${l.path}`;
    }
    return undefined;
  };
  const links = [copy.link, copy.extra].filter((l): l is ErrorLink => !!l && (!l.ownerOnly || canOwn));
  return (
    <div role="alert" className="max-w-chat-card rounded-card rounded-tl-sm border border-danger-ink/30 bg-danger-soft px-3.5 py-2.5 text-sm text-danger-ink">
      <p className="break-words">
        {copy.message}
        {links.map((l) => {
          const href = hrefOf(l);
          return href ? (
            <span key={l.label}>
              {" "}
              <Link href={href} className="font-semibold underline">
                {l.label}
              </Link>
            </span>
          ) : null;
        })}
        {copy.guideStep ? (
          <span>
            {" "}
            <Link href={`${base}/welcome?step=${copy.guideStep}`} className="font-semibold underline">
              Fix it in the guide
            </Link>
          </span>
        ) : null}
        {copy.link && copy.link.path === "computer" ? <span> (see the Computer tab)</span> : null}
      </p>
      {/* (chat-7) One red box; the technical line is under Details, never in the sentence. */}
      {technical ? (
        <div className="mt-1">
          <button
            type="button"
            aria-expanded={details}
            onClick={() => setDetails((d) => !d)}
            className="inline-flex min-h-11 items-center text-sm underline md:min-h-0">
            {details ? "Hide details" : "Details"}
          </button>
          {details ? <p className="mt-1 break-words rounded-control bg-canvas px-3 py-2 font-mono text-xs text-ink">{technical}</p> : null}
        </div>
      ) : null}
      {children}
    </div>
  );
}

/** The start-failure codes of C-581/C-585 whose failed start the retry endpoint accepts (C-610). */
const RETRYABLE_CODES = new Set([
  "sandbox_failed",
  "start_failed",
  "sandbox_start_failed",
  "supervisor_unreachable",
  "supervisor_boot_failed",
  "runner_unavailable",
  "pod_refused",
  "pod_failed",
  "home_volume_unbound",
  "image_unavailable",
  "harness_unsupported",
  "agent_start_failed",
  "at_capacity",
]);

/** The code maps to a start failure the retry endpoint accepts (C-585, C-610). */
export const isRetryableCode = (code: string | null | undefined): boolean => !!code && RETRYABLE_CODES.has(code);

/**
 * (FIX-029, C-615) **Retry now**: calls C-610, then reads "Start in flight…" until the sandbox
 * leaves `failed` (the sandbox query polls every 5 s). Hidden when the sandbox is not retryable
 * (another state) or the agent is frozen (C-610 would answer 423). `onRetried` fires after a 202 —
 * the chat re-posts the failed turn's text (C-615).
 */
export function RetryStart({
  agentId,
  onRetried,
  onError,
}: {
  agentId: string;
  /** Called after a 202 (the start arm claims on the next runner poll). */
  onRetried?: () => void;
  /** Called with the endpoint's error (409 `sandbox_not_failed`, 423 `frozen`, 404). */
  onError?: (e: unknown) => void;
}) {
  const q = useSandbox(agentId);
  const retry = useRetrySandbox(agentId);
  const [err, setErr] = useState<string | null>(null);
  // `retryable` is true only while the state is `failed`; while the retry is in flight the button
  // is disabled ("Start in flight…"), and once the sandbox has left `failed` it goes away —
  // there is nothing left to retry (C-615).
  const retryable = q.data?.retryable ?? false;
  const fail = (e: unknown) => {
    const msg = e instanceof ApiError ? e.message : "Could not retry the start.";
    setErr(msg);
    onError?.(e);
  };
  // The sandbox query failed (e.g. a member without view rights): hide rather than mislead.
  if (q.isError || (!retryable && !retry.isPending)) return null;
  return (
    <span className="mt-1.5 flex flex-wrap items-center gap-2">
      <Button
        size="sm"
        variant="outline"
        disabled={retry.isPending}
        onClick={() => {
          setErr(null);
          retry.mutate(undefined, { onSuccess: () => onRetried?.(), onError: fail });
        }}>
        {retry.isPending ? "Start in flight…" : "Retry now"}
      </Button>
      {err ? (
        <span role="alert" className="text-xs">
          {err}
        </span>
      ) : null}
    </span>
  );
}

/**
 * (FIX-225, D-470) What an API refusal on a queued row says, in words. The codes the follow-up routes answer have
 * their own copy; any other code falls back to the API's own message.
 */
export function followupErrorText(e: unknown): string {
  if (e instanceof ApiError) {
    switch (e.code) {
      case "followup_not_waiting":
        return "This message was already handed to the agent, cancelled or run, so it can't be changed.";
      case "steer_unsupported":
        return "This agent can't take messages while it works. The message runs when the current turn ends.";
      case "send_now_unsupported":
        return "This agent can't move a running step to the background.";
      case "followup_forbidden":
        return "Only the person who wrote this message can edit it.";
      default:
        return e.message;
    }
  }
  return "Something went wrong. Try again.";
}

type Pending = "steer" | "send_now" | "edit" | "cancel";

/**
 * (FIX-225, D-469, D-470, C-1154) A queued follow-up above the composer: a dashed row with a Queued badge, the text on one
 * line and exactly the buttons the server says the caller may use now (`followup.controls`): **Steer** or **Send now**,
 * **Edit**, **Cancel**. A button is never shown because the action "might" work. Every click answers: the row changes, or
 * the refusal is written on the row (`role="alert"`). After Steer it reads "Steering in at the next step…". Edit turns
 * the row into a text box with Save and Discard changes. `notice` is an outcome that arrived later (a send-now that found
 * nothing running); the row also says when the harness missed a steer.
 */
export function QueuedRow({
  followup,
  notice,
  onSteer,
  onSendNow,
  onSave,
  onCancel,
}: {
  followup: Followup;
  /** A later outcome for this row's turn, e.g. "Nothing was running to move to the background." */
  notice?: string | null;
  onSteer: () => Promise<unknown>;
  onSendNow: () => Promise<unknown>;
  /** Save the edited text. */
  onSave: (text: string) => Promise<unknown>;
  onCancel: () => Promise<unknown>;
}) {
  const [busy, setBusy] = useState<Pending | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(followup.text);
  // The server's `steer_requested` arrives with the next refetch; until then this click's own answer shows.
  const [asked, setAsked] = useState(false);
  const [gone, setGone] = useState(false);
  const c = followup.controls;
  const steering = (followup.steer_requested || asked) && !followup.steer_error;

  async function run(kind: Pending, fn: () => Promise<unknown>, done?: () => void) {
    setBusy(kind);
    setError(null);
    try {
      await fn();
      done?.();
    } catch (e) {
      setError(followupErrorText(e));
    } finally {
      setBusy(null);
    }
  }
  if (gone) return null;
  const btn = "max-md:min-h-11";
  const failed =
    followup.steer_error === "refused"
      ? "Couldn't steer this in (the agent can't take messages right now). It will run after the current turn."
      : followup.steer_error
        ? "Couldn't steer this in (the agent missed it). It will run after the current turn."
        : null;
  return (
    <div className="rounded-control border border-dashed border-line-strong px-2.5 py-1.5 text-sm">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
        <Badge>Queued</Badge>
        {editing ? null : (
          <span className="min-w-0 flex-1 basis-40 truncate text-ink-2" title={followup.text}>
            {followup.text}
          </span>
        )}
        {editing ? null : (
          <span className="flex w-full flex-wrap gap-2 sm:w-auto">
            {c.steer && !steering ? (
              <Button size="sm" variant="outline" className={btn} disabled={busy !== null} onClick={() => void run("steer", onSteer, () => setAsked(true))}>
                Steer
              </Button>
            ) : null}
            {c.send_now ? (
              <Button size="sm" variant="outline" className={btn} disabled={busy !== null} onClick={() => void run("send_now", onSendNow)}>
                Send now
              </Button>
            ) : null}
            {c.edit ? (
              <Button
                size="sm"
                variant="outline"
                className={btn}
                disabled={busy !== null}
                onClick={() => {
                  setDraft(followup.text);
                  setError(null);
                  setEditing(true);
                }}>
                Edit
              </Button>
            ) : null}
            {c.cancel ? (
              <Button size="sm" variant="outline" className={btn} disabled={busy !== null} onClick={() => void run("cancel", onCancel, () => setGone(true))}>
                Cancel
              </Button>
            ) : null}
          </span>
        )}
      </div>
      {editing ? (
        <div className="mt-1.5 space-y-2">
          <textarea
            aria-label="Edit queued message"
            rows={3}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            className="block max-h-44 min-h-16 w-full min-w-0 resize-y rounded-control border border-line-strong bg-surface px-2 py-1.5 text-base text-ink focus-visible:border-accent focus-visible:outline-none md:text-sm"
          />
          <span className="flex flex-wrap gap-2">
            <Button
              size="sm"
              className={btn}
              disabled={busy !== null || !draft.trim()}
              onClick={() =>
                void run("edit", () => onSave(draft.trim()), () => {
                  setEditing(false);
                })
              }>
              {busy === "edit" ? "Saving…" : "Save"}
            </Button>
            <Button
              size="sm"
              variant="outline"
              className={btn}
              disabled={busy !== null}
              onClick={() => {
                setEditing(false);
                setError(null);
              }}>
              Discard changes
            </Button>
          </span>
        </div>
      ) : null}
      {steering ? (
        <p role="status" className="mt-1 text-xs text-ink-2">
          Steering in at the next step…
        </p>
      ) : null}
      {failed ? (
        <p role="alert" className="mt-1 text-xs text-danger-ink">
          {failed}
        </p>
      ) : null}
      {notice ? (
        <p role="alert" className="mt-1 text-xs text-danger-ink">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="mt-1 text-xs text-danger-ink">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** The automated-turn-limit notice: a notice row in the thread with a Continue button. */
export function ContinueNotice({ text, convId }: { text: string; convId: string }) {
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  return (
    <Notice className="mx-auto flex max-w-chat-notice flex-wrap items-center gap-2">
      <p className="min-w-0 flex-1">{text || "The agent paused after reaching the automated turn limit."}</p>
      <Button
        size="sm"
        variant="outline"
        disabled={busy}
        onClick={() => {
          setBusy(true);
          setErr(null);
          chatApi
            .continue(convId)
            // The durable `app_turn_cap_reset` notice arrives on the transcript stream and hides this button.
            .catch(setErr)
            .finally(() => setBusy(false));
        }}>
        Continue
      </Button>
      {err ? (
        <p role="alert" className="w-full text-danger-ink">
          {err instanceof ApiError ? err.message : "Could not continue."}
        </p>
      ) : null}
    </Notice>
  );
}

/** A platform notice or a small system line (follow-up joined, correction). */
export function SystemRow({ children }: { children: React.ReactNode }) {
  return <p className="mx-auto max-w-chat-notice rounded-control bg-subtle px-3 py-1.5 text-center text-xs text-ink-2">{children}</p>;
}

/**
 * (FIX-253, D-490) The **Background activity** notice: what the harness child keeps doing after
 * the turn ended, with Stop. The row stays (the transcript renders it muted once the durable
 * `background_stopped` notice exists); the button is only for a still-running entry.
 */
export function BackgroundActivityNotice({ text, turnId }: { text: string; turnId: string }) {
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  return (
    <Notice className="mx-auto flex max-w-chat-notice flex-wrap items-center gap-2" data-testid="background-activity">
      <p className="min-w-0 flex-1">{text}</p>
      <Button
        size="sm"
        variant="outline"
        disabled={busy}
        onClick={() => {
          setBusy(true);
          setErr(null);
          chatApi
            .abort(turnId)
            .catch(setErr)
            .finally(() => setBusy(false));
        }}>
        Stop
      </Button>
      {err ? (
        <p role="alert" className="w-full text-danger-ink">
          {err instanceof ApiError ? err.message : "Could not stop the background activity."}
        </p>
      ) : null}
    </Notice>
  );
}
