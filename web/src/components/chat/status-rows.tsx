"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useState } from "react";

import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { chatApi, useRetrySandbox, useSandbox } from "@/lib/api/conversations";
import { errorCopy, type ErrorLink } from "@/lib/chat/transcript";

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
  base,
  canOwn = false,
  children,
}: {
  code?: string | null;
  message?: string;
  base: string;
  /** Workspace owner: gets the "Agent access" link. */
  canOwn?: boolean;
  /** Reserved (FIX-029, C-615): the retry button is passed as `children` by the chat's error blocks. */
  children?: React.ReactNode;
}) {
  const copy = errorCopy(code, message);
  // The chat lives on the agent page, so its tabs are addressed relative to the current path (`?tab=`, C-506).
  const pathname = usePathname() ?? "";
  const hrefOf = (l: ErrorLink): string | undefined =>
    l.path === "credentials"
      ? `${base}/credentials${l.tab ? `?tab=${l.tab}` : ""}`
      : l.path === "access" || l.path === "instructions"
        ? `${pathname}?tab=${l.path}`
        : undefined;
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
      {copy.showServerMessage && message ? <p className="mt-1 break-words text-xs">{message}</p> : null}
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
        }}
      >
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

/** A queued follow-up: a dashed row above the composer with a QUEUED badge, one line of text and "Send now". */
export function QueuedRow({ text, onSendNow }: { text: string; onSendNow: () => void }) {
  return (
    <div className="flex min-h-11 items-center gap-2 rounded-control border border-dashed border-line-strong px-2.5 py-1 text-sm">
      <Badge tone="pending">Queued</Badge>
      <span className="min-w-0 flex-1 truncate text-ink-2" title={text}>
        Queued follow-up: {text}
      </span>
      <Button size="sm" variant="outline" onClick={onSendNow}>
        Send now
      </Button>
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
        }}
      >
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
