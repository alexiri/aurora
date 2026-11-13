"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";

import { Button } from "@/components/ui/button";
import { PaperclipIcon, SendIcon, StopIcon } from "@/components/ui/icons";
import { ApiError } from "@/lib/api/client";
import { uploadAttachment, uploadErrorMessage, type Attachment } from "@/lib/api/attachments";
import { chatApi, type Followup } from "@/lib/api/conversations";
import { newIdempotencyKey } from "@/lib/chat/idempotency";
import { classifySend, pendingFollowups, type TranscriptState } from "@/lib/chat/transcript";

import { ComposerChip, UploadChip } from "./attachments";
import { ErrorBlock, QueuedRow } from "./status-rows";

/**
 * The pinned composer: queued follow-ups above the box, then a bordered box with attachment chips, an
 * auto-growing textarea (2 to 8 lines, CSS only) and one toolbar row (Attach, the hint, Stop, Send). At 390 px it
 * is a single row of 44 px controls. Enter sends, Shift+Enter adds a new line.
 */
export function Composer({
  agentId,
  convId,
  base,
  agentName,
  activeTurn,
  state,
  autoFocus = false,
}: {
  agentId: string;
  convId: string;
  base: string;
  agentName: string;
  /** Id of the turn in flight, or `null`. */
  activeTurn: string | null;
  state: TranscriptState;
  /** Focus the box (an empty conversation). */
  autoFocus?: boolean;
}) {
  const qc = useQueryClient();
  // `?draft=` fills the box (the getting-started guide suggests a first message); it is never sent for the person.
  // Read at mount regardless of `autoFocus`: on a slow load the composer mounts while the transcript is still
  // loading (`autoFocus` false), and the initializer would never run again — the guide's suggested message was
  // lost. The param only exists on links from the guide, and the box is empty on every other path, so an
  // unconditional read cannot clobber a typed message.
  const [text, setText] = useState(() => (typeof window !== "undefined" ? (new URLSearchParams(window.location.search).get("draft") ?? "").slice(0, 500) : ""));
  const [followups, setFollowups] = useState<Followup[]>([]);
  const [sendErr, setSendErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState<string[]>([]);
  const [uploadErr, setUploadErr] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const keyRef = useRef<{ sig: string; key: string } | null>(null);
  const waiting = useMemo(() => pendingFollowups(followups, state), [followups, state]);

  useEffect(() => {
    if (autoFocus) area.current?.focus();
  }, [autoFocus]);

  // The draft is a one-shot suggestion (WEBA-20): once it has filled the box, drop `?draft=` from the URL so a
  // remount (switching conversations and back, a re-render of the page) does not put it back.
  useEffect(() => {
    const url = new URL(window.location.href);
    if (!url.searchParams.has("draft")) return;
    url.searchParams.delete("draft");
    window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
  }, []);

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    const t = text.trim();
    if (!t || busy || uploading.length > 0) return;
    setBusy(true);
    setSendErr(null);
    // One key per message (RFE-018): a retry of the same text and attachments reuses it, so a lost response never
    // makes a second turn; an edited message gets a fresh one (the server refuses a key reused with other content).
    const ids = pending.map((a) => a.id);
    const sig = JSON.stringify([convId, t, [...ids].sort()]);
    if (keyRef.current?.sig !== sig) keyRef.current = { sig, key: newIdempotencyKey() };
    try {
      const out = classifySend(await chatApi.send(convId, t, ids, keyRef.current.key));
      keyRef.current = null;
      if (out.kind === "followup") setFollowups((f) => [...f, out.followup]);
      setText("");
      setPending([]);
      void qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations"] });
    } catch (err) {
      // The server took the key for other content: never reuse it.
      if (err instanceof ApiError && err.code === "idempotency_key_reused") keyRef.current = null;
      setSendErr(err);
    } finally {
      setBusy(false);
    }
  }

  async function attach(files: FileList | null) {
    if (!files) return;
    setUploadErr(null);
    for (const file of Array.from(files)) {
      setUploading((n) => [...n, file.name]);
      try {
        const a = await uploadAttachment(convId, file);
        setPending((p) => [...p, a]);
      } catch (err) {
        setUploadErr(`Upload failed: ${file.name}: ${uploadErrorMessage(err)}`);
      } finally {
        setUploading((n) => {
          const i = n.indexOf(file.name);
          return i < 0 ? n : [...n.slice(0, i), ...n.slice(i + 1)];
        });
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

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit();
    }
  }

  const tooMany = sendErr instanceof ApiError && sendErr.code === "too_many_attachments";
  return (
    <div className="shrink-0 space-y-2 border-t border-line bg-surface p-3 dark:bg-canvas md:px-5">
      {waiting.map((f) => (
        <QueuedRow key={f.id} text={f.text} onSendNow={() => void act(() => chatApi.sendNow(f.parent_turn_id))} />
      ))}
      {tooMany ? (
        <p role="alert" className="text-sm text-danger-ink">
          {uploadErrorMessage(sendErr)}
        </p>
      ) : sendErr ? (
        <ErrorBlock
          code={sendErr instanceof ApiError ? sendErr.code : null}
          message={sendErr instanceof ApiError ? sendErr.message : undefined}
          base={base}
        />
      ) : null}
      <form
        onSubmit={(e) => void submit(e)}
        className="rounded-card border border-line-strong bg-surface p-2 focus-within:border-accent dark:bg-canvas md:p-2.5"
      >
        {pending.length > 0 ? (
          <ul aria-label="Pending attachments" className="mb-2 flex flex-wrap gap-2">
            {pending.map((a) => (
              <ComposerChip key={a.id} attachment={a} onRemove={() => setPending((p) => p.filter((x) => x.id !== a.id))} />
            ))}
          </ul>
        ) : null}
        {uploading.length > 0 || uploadErr ? (
          <div className="mb-2 flex flex-wrap gap-2">
            {uploading.map((n, i) => (
              <UploadChip key={`${n}-${i}`} name={n} />
            ))}
            {uploadErr ? <UploadChip name="" error={uploadErr} /> : null}
          </div>
        ) : null}
        <div className="flex flex-wrap items-end gap-1 md:gap-1.5">
          <label htmlFor="chat-input" className="sr-only">
            Message
          </label>
          <textarea
            id="chat-input"
            ref={area}
            rows={1}
            value={text}
            placeholder={activeTurn ? "Add a follow-up…" : `Message ${agentName}…`}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            className="order-2 block max-h-44 min-h-11 min-w-0 flex-1 resize-none rounded-control bg-transparent px-2 py-2.5 text-sm text-ink [field-sizing:content] placeholder:text-muted focus-visible:shadow-none focus-visible:outline-none md:order-1 md:min-h-14 md:basis-full md:py-1.5"
          />
          <label className="order-1 inline-flex size-11 shrink-0 cursor-pointer items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink focus-within:outline focus-within:outline-2 focus-within:outline-accent md:order-2 md:size-9">
            <PaperclipIcon width={18} height={18} />
            <span className="sr-only">Attach</span>
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
          <span className="order-3 hidden flex-1 px-1 text-xs text-muted md:block">Enter to send · Shift+Enter for a new line</span>
          {activeTurn ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              aria-label="Stop"
              className="order-4 size-11 min-h-11 shrink-0 px-0 md:w-auto md:min-h-9 md:px-3"
              onClick={() => void act(() => chatApi.abort(activeTurn))}
            >
              <StopIcon width={16} height={16} />
              <span className="hidden md:inline">Stop</span>
            </Button>
          ) : null}
          <Button
            type="submit"
            size="lg"
            className="order-5 size-11 min-h-11 shrink-0 px-0"
            aria-label={activeTurn ? "Follow up" : "Send"}
            disabled={busy || uploading.length > 0 || !text.trim()}
          >
            <SendIcon width={18} height={18} />
          </Button>
        </div>
      </form>
    </div>
  );
}
