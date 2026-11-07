"use client";

import { useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { useMemo, useState, type FormEvent } from "react";

import { Markdown } from "@/components/apps/markdown";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { useRetention } from "@/lib/api/org";
import { useCanApprove, useDecide } from "@/lib/api/approvals";
import { ApiError } from "@/lib/api/client";
import { attachmentUrl, canThumbnail, formatBytes, uploadAttachment, uploadErrorMessage, type Attachment } from "@/lib/api/attachments";
import { chatApi, useConversations, useDirectConversation, type Conversation, type Followup, type TranscriptEvent } from "@/lib/api/conversations";
import {
  activeTurnId,
  approvalStatus,
  classifySend,
  errorCopy,
  pendingFollowups,
  type TranscriptState,
} from "@/lib/chat/transcript";
import { useTranscript } from "@/lib/chat/use-transcript";

import { ConversationActions, RedactButton } from "./conversation-actions";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

/**
 * Download link for an attachment. Content is never rendered inline (no HTML/SVG): the only embedded
 * form is an <img> of the download URL for raster images.
 */
export function AttachmentLink({ id, filename, contentType, size }: { id: string; filename?: string; contentType?: string; size?: number }) {
  const url = attachmentUrl(id);
  return (
    <span className="flex items-center gap-2 text-sm">
      {canThumbnail(contentType) ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={url} alt="" className="h-12 w-12 rounded border border-neutral-200 object-cover" />
      ) : null}
      <a className="underline" href={url} download={filename || undefined}>
        {filename || "Attachment"}
      </a>
      {size !== undefined ? <span className="text-xs text-neutral-600">({formatBytes(size)})</span> : null}
    </span>
  );
}

function ContinueNotice({ text, convId }: { text: string; convId: string }) {
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  return (
    <div className="space-y-1 rounded bg-blue-50 p-2 text-sm text-blue-900">
      <p>{text || "The agent paused after reaching the automated turn limit."}</p>
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
        <p role="alert" className="text-red-700">
          {err instanceof ApiError ? err.message : "Could not continue."}
        </p>
      ) : null}
    </div>
  );
}

export function ErrorNotice({ code, message, base }: { code?: string | null; message?: string; base: string }) {
  const copy = errorCopy(code, message);
  const href = copy.link ? (copy.link.path === "credentials" ? `${base}/credentials` : undefined) : undefined;
  return (
    <p role="alert" className="text-sm text-red-700">
      {copy.message}{" "}
      {copy.link && href ? (
        <Link href={href} className="underline">
          {copy.link.label}
        </Link>
      ) : null}
      {copy.link && copy.link.path === "computer" ? <span>(see the Computer tab)</span> : null}
    </p>
  );
}

export function ApprovalCard({ ev, state, wsId }: { ev: TranscriptEvent; state: TranscriptState; wsId: string }) {
  const id = s(ev.payload.approval_id);
  const st = approvalStatus(state, id);
  const decide = useDecide(wsId);
  const canApprove = useCanApprove(wsId);
  const settled = st.status !== "pending";
  return (
    <div className="rounded-md border border-amber-300 bg-amber-50 p-3 text-sm">
      <p className="font-medium">Approval requested</p>
      <p>{s(ev.payload.summary)}</p>
      {settled ? (
        <p role="status" className="mt-1 text-neutral-700">
          {st.status === "expired" ? "Expired" : st.status === "approve" ? "Approved" : "Denied"}
          {st.by ? ` by ${st.by}` : ""}
        </p>
      ) : !canApprove ? (
        <p role="status" className="mt-1 text-neutral-700">
          Waiting for an approver
        </p>
      ) : (
        <div className="mt-2 flex gap-2">
          <Button size="sm" disabled={decide.isPending} onClick={() => decide.mutate({ id, decision: "approve" })}>
            Approve
          </Button>
          <Button size="sm" variant="outline" disabled={decide.isPending} onClick={() => decide.mutate({ id, decision: "deny" })}>
            Deny
          </Button>
        </div>
      )}
      {decide.error ? (
        <p role="alert" className="mt-1 text-red-700">
          {decide.error instanceof ApiError && decide.error.status === 409 ? "Already decided." : "Could not record the decision."}
        </p>
      ) : null}
    </div>
  );
}

function ActionApprovalNote({ summary, wsId }: { summary: string; wsId: string }) {
  const canApprove = useCanApprove(wsId);
  return (
    <p className="rounded-md border border-amber-300 bg-amber-50 p-2 text-sm">
      Action approval: {summary} {canApprove ? "(decide in Approvals)" : "(waiting for an approver)"}
    </p>
  );
}

export function EventView({ ev, state, wsId, base, convId }: { ev: TranscriptEvent; state: TranscriptState; wsId: string; base: string; convId: string }) {
  const p = ev.payload;
  if (p.redacted === true) {
    // RFE-010: the content was replaced in place; only the marker remains.
    return (
      <p className="text-xs italic text-neutral-600">
        Content redacted{s(p.redacted_by) ? ` by ${s(p.redacted_by)}` : ""}
        {s(p.reason) ? ` (${s(p.reason)})` : ""}.
      </p>
    );
  }
  switch (ev.type) {
    case "turn.queued":
      return (
        <div className="ml-auto max-w-[80%] rounded-md bg-neutral-100 p-2 text-sm">
          <p className="text-xs text-neutral-600">
            {s(p.speaker_label) || "You"}
            {s(p.door) && s(p.door) !== "dashboard" ? (
              <span className="ml-2 rounded-full border border-neutral-300 px-2 py-0.5">{s(p.door)}</span>
            ) : null}
          </p>
          <p className="whitespace-pre-wrap">{s(p.text)}</p>
          {Array.isArray(p.attachments)
            ? (p.attachments as unknown[]).map((a) => <AttachmentLink key={s(a)} id={s(a)} />)
            : null}
        </div>
      );
    case "text":
      return (
        <div className="max-w-[90%] rounded-md border border-neutral-200 p-2">
          <Markdown source={s(p.text)} />
        </div>
      );
    case "tool.started": {
      const fin = state.events.find((e) => e.type === "tool.finished" && s(e.payload.call_id) === s(p.call_id));
      return (
        <div className="rounded-md border border-neutral-200 p-2 text-xs">
          <span className="font-mono font-medium">{s(p.name)}</span> <span>{s(p.summary)}</span>{" "}
          <span className="text-neutral-600">
            {fin ? (fin.payload.ok ? "done" : "failed") : "running…"}
            {fin && s(fin.payload.summary) ? `: ${s(fin.payload.summary)}` : ""}
          </span>
        </div>
      );
    }
    case "correction":
      return (
        <p className="text-xs italic text-neutral-600">
          {p.action === "retract" ? "A previous message was retracted." : `A previous message was corrected: ${s(p.text)}`}
        </p>
      );
    case "message.joined":
      return <p className="text-xs text-neutral-600">Follow-up joined the running turn.</p>;
    case "message.sent_now":
      return <p className="text-xs text-neutral-600">Follow-up sent now.</p>;
    case "approval.pending":
      return <ApprovalCard ev={ev} state={state} wsId={wsId} />;
    case "action_approval.pending":
      return <ActionApprovalNote summary={s(p.summary)} wsId={wsId} />;
    case "attachment":
      return (
        <AttachmentLink
          id={s(p.attachment_id)}
          filename={s(p.filename)}
          contentType={s(p.content_type)}
          size={typeof p.size === "number" ? p.size : typeof p.size_bytes === "number" ? p.size_bytes : undefined}
        />
      );
    case "notice": {
      if (s(p.code) === "app_turn_cap_reached") {
        // Only offer Continue until a later durable reset notice (C-27) exists.
        const reset = state.events.some((e) => (e.seq ?? 0) > (ev.seq ?? 0) && e.type === "notice" && s(e.payload.code) === "app_turn_cap_reset");
        if (!reset) return <ContinueNotice text={s(p.text)} convId={convId} />;
      }
      return <p className="rounded bg-blue-50 p-2 text-sm text-blue-900">{s(p.text)}</p>;
    }
    case "error":
      return <ErrorNotice code={s(p.code)} message={s(p.message)} base={base} />;
    case "turn.finished":
      return s(p.status) === "failed" && s(p.error_code) ? <ErrorNotice code={s(p.error_code)} base={base} /> : null;
    default:
      return null; // unknown or non-rendered types
  }
}

function Transcript({ state, wsId, base, convId }: { state: TranscriptState; wsId: string; base: string; convId: string }) {
  const turn = activeTurnId(state);
  const deltas = Object.values(state.deltas);
  return (
    <div className="space-y-2" aria-live="polite">
      {state.events.map((ev) => (
        <div key={ev.seq}>
          <EventView ev={ev} state={state} wsId={wsId} base={base} convId={convId} />
          {(ev.type === "turn.queued" || ev.type === "text") && ev.payload.redacted !== true && ev.seq ? (
            <div className={ev.type === "turn.queued" ? "text-right" : ""}>
              <RedactButton convId={convId} seq={ev.seq} />
            </div>
          ) : null}
        </div>
      ))}
      {deltas.map((d) => (
        <div key={d.messageId} className="max-w-[90%] rounded-md border border-dashed border-neutral-300 p-2">
          <Markdown source={d.text} />
        </div>
      ))}
      {turn && state.activity[turn] ? <p className="text-xs text-neutral-500">{state.activity[turn]}</p> : null}
    </div>
  );
}

export function ChatTab({ agentId, wsId, base, orgId }: { agentId: string; wsId: string; base: string; orgId?: string }) {
  const direct = useDirectConversation(agentId);
  const [showArchived, setShowArchived] = useState(false);
  const list = useConversations(agentId, showArchived);
  const retention = useRetention(orgId);
  const [picked, setPicked] = useState<string | null>(null);
  if (direct.isPending) return <Loading />;
  if (direct.isError) return <ErrorState error={direct.error} onRetry={() => void direct.refetch()} />;
  const convId = picked ?? direct.data.id;
  const convs = list.data?.items ?? [];
  // RFE-009: other members' direct conversations (owners only; the API returns them with
  // `direct_user`) are listed apart, read-only, and marked as visible to workspace owners.
  const others = showArchived ? [] : convs.filter((c) => c.id !== direct.data.id && c.kind !== "direct");
  const memberDirects = showArchived ? [] : convs.filter((c) => c.id !== direct.data.id && c.kind === "direct" && c.direct_user);
  const archivedItems = showArchived ? convs : [];
  const days = retention.data?.conversations_days ?? null;
  const pickedDirect = memberDirects.find((c) => c.id === picked) ?? null;
  return (
    <div className="space-y-3">
      {days ? (
        <p role="note" className="rounded bg-neutral-100 p-2 text-xs text-neutral-700">
          Your organization deletes conversations after {days} days without activity.
        </p>
      ) : null}
    <div className="grid gap-4 md:grid-cols-[14rem_1fr]">
      <nav aria-label="Conversations" className="space-y-1 text-sm">
        <button
          type="button"
          aria-pressed={showArchived}
          onClick={() => {
            setShowArchived((v) => !v);
            setPicked(null);
          }}
          className="block w-full rounded border border-neutral-300 px-2 py-1 text-left text-xs"
        >
          {showArchived ? "Back to conversations" : "Archived"}
        </button>
        {showArchived && archivedItems.length === 0 ? <p className="px-2 text-xs text-neutral-600">No archived conversations.</p> : null}
        {archivedItems.map((c) => (
          <button
            key={c.id}
            type="button"
            onClick={() => setPicked(c.id)}
            aria-current={picked === c.id}
            className={`block w-full rounded px-2 py-1 text-left ${picked === c.id ? "bg-neutral-100 font-medium" : ""}`}
          >
            <span className="mr-1 rounded-full border border-neutral-300 px-1.5 text-xs">{c.kind}</span>
            {c.title ?? c.external_ref}
          </button>
        ))}
        {showArchived ? null : (
          <button
            type="button"
            onClick={() => setPicked(null)}
            aria-current={!picked}
            className={`block w-full rounded px-2 py-1 text-left ${!picked ? "bg-neutral-100 font-medium" : ""}`}
          >
            Direct chat
          </button>
        )}
        {others.map((c) => (
          <button
            key={c.id}
            type="button"
            onClick={() => setPicked(c.id)}
            aria-current={picked === c.id}
            className={`block w-full rounded px-2 py-1 text-left ${picked === c.id ? "bg-neutral-100 font-medium" : ""}`}
          >
            <span className="mr-1 rounded-full border border-neutral-300 px-1.5 text-xs">{c.kind}</span>
            {c.title ?? c.external_ref}
          </button>
        ))}
        {memberDirects.length > 0 ? (
          <div role="group" aria-label="Direct conversations of other members" className="space-y-1 pt-2">
            <p className="px-2 text-xs font-medium text-neutral-600">Direct conversations of other members</p>
            {memberDirects.map((c) => (
              <button
                key={c.id}
                type="button"
                onClick={() => setPicked(c.id)}
                aria-current={picked === c.id}
                className={`block w-full rounded px-2 py-1 text-left ${picked === c.id ? "bg-neutral-100 font-medium" : ""}`}
              >
                {c.direct_user?.display_name}
                <span className="block text-xs text-neutral-600">visible to workspace owners</span>
              </button>
            ))}
          </div>
        ) : null}
      </nav>
      {showArchived && !picked ? (
        <EmptyState title="Archived conversations">Pick one to read it, export it or unarchive it.</EmptyState>
      ) : (
        <ChatPane
          key={convId}
          agentId={agentId}
          wsId={wsId}
          base={base}
          convId={convId}
          direct={convId === direct.data.id ? direct.data : null}
          ownerView={pickedDirect?.direct_user?.display_name ?? null}
          ownDirect={convId === direct.data.id}
          archived={showArchived}
          onGone={() => {
            setPicked(null);
            setShowArchived(false);
          }}
        />
      )}
    </div>
    </div>
  );
}

export function ChatPane({
  agentId,
  wsId,
  base,
  convId,
  direct,
  ownerView = null,
  ownDirect = false,
  archived = false,
  onGone,
}: {
  agentId: string;
  wsId: string;
  base: string;
  convId: string;
  direct: Conversation | null;
  /** Display name of the person whose direct conversation an owner is reading (read-only). */
  ownerView?: string | null;
  ownDirect?: boolean;
  /** The conversation is archived (RFE-010): offer Unarchive instead of Archive. */
  archived?: boolean;
  onGone?: () => void;
}) {
  const qc = useQueryClient();
  const { state, loading, error, live } = useTranscript(convId);
  const [text, setText] = useState("");
  const [followups, setFollowups] = useState<Followup[]>([]);
  const [sendErr, setSendErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState(0);
  const [uploadErr, setUploadErr] = useState<string | null>(null);
  const active = activeTurnId(state, direct);
  const waiting = useMemo(() => pendingFollowups(followups, state), [followups, state]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const t = text.trim();
    if (!t || !convId || busy || uploading > 0) return;
    setBusy(true);
    setSendErr(null);
    try {
      const out = classifySend(await chatApi.send(convId, t, pending.map((a) => a.id)));
      if (out.kind === "followup") setFollowups((f) => [...f, out.followup]);
      setText("");
      setPending([]);
      void qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations"] });
    } catch (err) {
      setSendErr(err);
    } finally {
      setBusy(false);
    }
  }

  async function attach(files: FileList | null) {
    if (!files) return;
    setUploadErr(null);
    for (const file of Array.from(files)) {
      setUploading((n) => n + 1);
      try {
        const a = await uploadAttachment(convId, file);
        setPending((p) => [...p, a]);
      } catch (err) {
        setUploadErr(`${file.name}: ${uploadErrorMessage(err)}`);
      } finally {
        setUploading((n) => n - 1);
      }
    }
  }

  async function act(fn: () => Promise<unknown>) {
    setSendErr(null);
    try {
      await fn();
    } catch (err) {
      setSendErr(err);
    }
  }

  return (
      <section aria-label="Chat" className="space-y-3">
        {onGone ? <ConversationActions agentId={agentId} convId={convId} archived={archived} onGone={onGone} /> : null}
        {ownDirect || direct ? (
          <p role="note" className="rounded bg-neutral-100 p-2 text-xs text-neutral-700">
            Only you and workspace owners can see this conversation.
          </p>
        ) : null}
        {ownerView !== null ? (
          <p role="note" className="rounded border border-amber-300 bg-amber-50 p-2 text-xs text-amber-900">
            This is {ownerView || "another member"}&apos;s direct conversation. You can read it because you own this workspace; your
            access is recorded in the audit log. You can stop its turns but cannot write in it.
          </p>
        ) : null}
        {loading ? <Loading /> : null}
        {error ? <ErrorState error={error} /> : null}
        {!loading && !error && state.events.length === 0 ? (
          <EmptyState title="No messages yet">Say hello to start the conversation.</EmptyState>
        ) : null}
        <Transcript state={state} wsId={wsId} base={base} convId={convId} />
        {!live && !loading && !error ? <p className="text-xs text-neutral-500">Reconnecting to live updates…</p> : null}
        {waiting.map((f) => (
          <div key={f.id} className="flex items-center gap-2 text-sm">
            <span className="text-neutral-600">Queued follow-up: {f.text}</span>
            <Button size="sm" variant="outline" onClick={() => void act(() => chatApi.sendNow(f.parent_turn_id))}>
              Send now
            </Button>
          </div>
        ))}
        {sendErr instanceof ApiError && sendErr.code === "too_many_attachments" ? (
          <p role="alert" className="text-sm text-red-700">
            {uploadErrorMessage(sendErr)}
          </p>
        ) : sendErr ? (
          <ErrorNotice
            code={sendErr instanceof ApiError ? sendErr.code : null}
            message={sendErr instanceof ApiError ? sendErr.message : undefined}
            base={base}
          />
        ) : null}
        {pending.length > 0 ? (
          <ul aria-label="Pending attachments" className="flex flex-wrap gap-2">
            {pending.map((a) => (
              <li key={a.id} className="flex items-center gap-1 rounded-full border border-neutral-300 px-2 py-0.5 text-xs">
                <span>
                  {a.filename} ({formatBytes(a.size_bytes)})
                </span>
                <button type="button" aria-label={`Remove ${a.filename}`} onClick={() => setPending((p) => p.filter((x) => x.id !== a.id))}>
                  ×
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        {uploading > 0 ? <p role="status" className="text-xs text-neutral-600">Uploading…</p> : null}
        {uploadErr ? (
          <p role="alert" className="text-sm text-red-700">
            {uploadErr}
          </p>
        ) : null}
        {ownerView !== null ? (
          active ? (
            <Button type="button" variant="outline" onClick={() => void act(() => chatApi.abort(active))}>
              Stop
            </Button>
          ) : null
        ) : (
        <form onSubmit={(e) => void submit(e)} className="flex gap-2">
          <label htmlFor="chat-input" className="sr-only">
            Message
          </label>
          <textarea
            id="chat-input"
            className={inputClass}
            rows={2}
            value={text}
            placeholder={active ? "Add a follow-up…" : "Message the agent…"}
            onChange={(e) => setText(e.target.value)}
          />
          <div className="flex flex-col gap-1">
            <label className="inline-flex h-9 cursor-pointer items-center justify-center rounded-md border border-neutral-300 px-3 text-sm focus-within:ring-2">
              Attach
              <input
                type="file"
                multiple
                className="sr-only"
                aria-label="Attach files"
                onChange={(e) => {
                  void attach(e.target.files);
                  e.target.value = "";
                }}
              />
            </label>
            <Button type="submit" disabled={busy || uploading > 0 || !text.trim()}>
              {active ? "Follow up" : "Send"}
            </Button>
            {active ? (
              <Button type="button" variant="outline" onClick={() => void act(() => chatApi.abort(active))}>
                Stop
              </Button>
            ) : null}
          </div>
        </form>
        )}
      </section>
  );
}
