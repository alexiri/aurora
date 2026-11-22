"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState, type ClipboardEvent, type FormEvent, type KeyboardEvent, type MutableRefObject } from "react";

import { Button } from "@/components/ui/button";
import { EmojiPicker } from "@/components/ui/emoji-picker";
import {
  BoldIcon,
  CodeBlockIcon,
  CodeIcon,
  HeadingIcon,
  ItalicIcon,
  LinkIcon,
  ListBulletIcon,
  ListNumberedIcon,
  PaperclipIcon,
  QuoteIcon,
  SendIcon,
  SmileIcon,
  StopIcon,
  StrikeIcon,
  TableIcon,
  TaskListIcon,
} from "@/components/ui/icons";
import { Markdown } from "@/components/ui/markdown";
import { Notice } from "@/components/ui/notice";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover";
import { SuggestionList, optionId } from "@/components/ui/suggestion-list";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { Kbd, Toolbar, ToolbarButton, ToolbarSeparator } from "@/components/ui/toolbar";
import { ApiError } from "@/lib/api/client";
import { uploadAttachment, uploadErrorMessage, type Attachment } from "@/lib/api/attachments";
import { chatApi, followupsKey, useFollowups } from "@/lib/api/conversations";
import { useMe } from "@/lib/api/queries";
import { newIdempotencyKey } from "@/lib/chat/idempotency";
import { markStopped, useStoppedTurns } from "@/lib/chat/stopped";
import { classifySend, followupSignal, sendNowFailures, type TranscriptState } from "@/lib/chat/transcript";

import { ComposerChip, UploadChip } from "./attachments";
import type { ReplyTarget } from "./chat-actions";
import { loadDraft, loadFormattingPref, saveDraft, saveFormattingPref } from "./composer/drafts";
import { applyEdit } from "./composer/edit";
import { searchEmoji, type EmojiHit } from "./composer/emoji";
import { SHORTCUTS, applyFormat, continueList, emojiQuery, inOpenFence, shortcutAction, smartPaste, type Edit, type FormatAction } from "./composer/format";
import { ReplyBar } from "./reply-bar";
import { ErrorBlock, QueuedRow } from "./status-rows";

/** `Pasted image 2026-11-14 23.37.09.png`: what the clipboard calls `image.png` is named by when it was pasted. */
export function pastedName(file: File, now = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  const ext = (file.type.split("/")[1] || "png").split("+")[0];
  return `Pasted image ${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())} ${p(now.getHours())}.${p(now.getMinutes())}.${p(now.getSeconds())}.${ext}`;
}

const ACTIONS: { action: FormatAction; label: string; icon: typeof BoldIcon; group: number }[] = [
  { action: "bold", label: "Bold", icon: BoldIcon, group: 0 },
  { action: "italic", label: "Italic", icon: ItalicIcon, group: 0 },
  { action: "strike", label: "Strikethrough", icon: StrikeIcon, group: 0 },
  { action: "heading", label: "Heading", icon: HeadingIcon, group: 0 },
  { action: "link", label: "Link", icon: LinkIcon, group: 1 },
  { action: "code", label: "Code", icon: CodeIcon, group: 1 },
  { action: "codeblock", label: "Code block", icon: CodeBlockIcon, group: 1 },
  { action: "quote", label: "Quote", icon: QuoteIcon, group: 2 },
  { action: "bullet", label: "Bulleted list", icon: ListBulletIcon, group: 2 },
  { action: "numbered", label: "Numbered list", icon: ListNumberedIcon, group: 2 },
  { action: "task", label: "Task list", icon: TaskListIcon, group: 2 },
  { action: "table", label: "Table", icon: TableIcon, group: 3 },
];

const isMac = () => typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

/**
 * The pinned composer: queued follow-ups above the box, then a bordered box with the reply bar, attachment chips,
 * the auto-growing text field (or its Preview), the formatting row and one action row (Attach, `Aa`, the hint, Stop,
 * Send). Markdown helpers, smart paste, emoji, files and drafts are P5-CHAT-3 (docs/14 "Composer (phase 5)").
 */
export function Composer({
  agentId,
  convId,
  base,
  agentName,
  activeTurn,
  state,
  autoFocus = false,
  reply = null,
  onClearReply,
  onRestoreReply,
  attachRef,
  initialDraft,
  onDraftUsed,
  onSent,
  draftRef,
}: {
  /** (item 9) Lets the thread put a suggested message in the box (focused, never sent). */
  draftRef?: MutableRefObject<((text: string) => void) | null>;
  /** (chat-2) The message was accepted: the transcript scrolls to the bottom. */
  onSent?: () => void;
  /** (P5-REFINE-13, C-1035) A link's suggested message (`?draft=`), already read from the router. Never sent for the person. */
  initialDraft?: string;
  /** The person edited the box or sent: the page may forget the draft. */
  onDraftUsed?: () => void;
  /** (P5-CHAT-2, C-725) The message this one replies to; sent as `reply_to.seq`. */
  reply?: ReplyTarget | null;
  onClearReply?: () => void;
  /** A draft's reply target, put back when the person returns. */
  onRestoreReply?: (t: ReplyTarget) => void;
  /** The thread pane's drop zone hands dropped files to this uploader. */
  attachRef?: MutableRefObject<((files: File[]) => void) | null>;
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
  const userId = useMe().data?.user.id ?? null;
  // (chat-4) Stop is disabled and says "Stopping…" while the call is in flight and until the turn finishes.
  const [stopBusy, setStopBusy] = useState(false);
  const stoppedTurns = useStoppedTurns();
  const stopping = stopBusy || (!!activeTurn && stoppedTurns.has(activeTurn));
  // `?draft=` fills the box (the getting-started guide suggests a first message); it is never sent for the person.
  // Read at mount regardless of `autoFocus`: on a slow load the composer mounts while the transcript is still
  // loading (`autoFocus` false), and the initializer would never run again — the guide's suggested message was
  // lost. The param only exists on links from the guide, and the box is empty on every other path, so an
  // unconditional read cannot clobber a typed message.
  // (P5-REFINE-13) The page hands the draft over as `initialDraft` (read from the router: during a client-side
  // navigation `window.location` still shows the previous page). The `window.location` read is the fallback.
  const [text, setText] = useState(() =>
    initialDraft !== undefined
      ? initialDraft.slice(0, 500)
      : typeof window !== "undefined"
        ? (new URLSearchParams(window.location.search).get("draft") ?? "").slice(0, 500)
        : "",
  );
  const [sendErr, setSendErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState<string[]>([]);
  const [uploadErrs, setUploadErrs] = useState<string[]>([]);
  const [expired, setExpired] = useState(0);
  const [mode, setMode] = useState<"write" | "preview">("write");
  const [fmtPref, setFmtPref] = useState<boolean | null>(null);
  const [caret, setCaret] = useState(0);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [hits, setHits] = useState<EmojiHit[]>([]);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const keyRef = useRef<{ sig: string; key: string } | null>(null);
  const restored = useRef<string | null>(null);
  // (FIX-225, D-469) The queued messages come from the server (rows survive a reload and everyone who can see the
  // conversation sees them); they are refetched whenever the stream says one was queued, joined, steered, edited,
  // cancelled, failed or run.
  const followupList = useFollowups(convId);
  const waiting = followupList.data?.items ?? [];
  const sendNowFailed = sendNowFailures(state);
  const signal = followupSignal(state);
  const refreshFollowups = () => qc.invalidateQueries({ queryKey: followupsKey(convId) });
  useEffect(() => {
    if (signal > 0) void qc.invalidateQueries({ queryKey: followupsKey(convId) });
  }, [signal, convId, qc]);
  const ids = useId().replace(/:/g, "");
  const listId = `${ids}-emoji`;

  useEffect(() => {
    if (autoFocus) area.current?.focus();
  }, [autoFocus]);
  // (P5-POLISH-5, C-1035) A `?draft=` suggestion fills the box and focuses it, never sent.
  const hadDraft = useRef(text.length > 0);
  useEffect(() => {
    if (hadDraft.current) area.current?.focus();
  }, []);
  // Choosing Reply moves focus to the text field.
  const replySeq = reply?.seq;
  useEffect(() => {
    if (replySeq) area.current?.focus();
  }, [replySeq]);

  // The draft is a one-shot suggestion (WEBA-20): once it has filled the box, drop `?draft=` from the URL so a
  // remount (switching conversations and back, a re-render of the page) does not put it back.
  useEffect(() => {
    const url = new URL(window.location.href);
    if (!url.searchParams.has("draft")) return;
    url.searchParams.delete("draft");
    window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
  }, []);

  // The remembered `Aa` choice.
  useEffect(() => {
    setFmtPref(loadFormattingPref());
  }, []);
  // (chat-8) One row until Aa is pressed, at every width; the choice is remembered.
  const fmtOn = fmtPref ?? false;

  // Drafts: restored once per user and conversation (a `?draft=` suggestion wins over a stored text), then saved
  // on every change; the saved copy is removed when everything is empty (after Send).
  useEffect(() => {
    if (!userId) return;
    const key = `${userId}:${convId}`;
    if (restored.current === key) return;
    restored.current = key;
    const d = loadDraft(userId, convId);
    if (!d) return;
    setText((t) => t || d.text);
    if (d.attachments.length > 0) setPending(d.attachments);
    if (d.reply) onRestoreReply?.({ seq: d.reply.seq, author: d.reply.author, excerpt: d.reply.excerpt });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [userId, convId]);
  useEffect(() => {
    if (!userId || restored.current !== `${userId}:${convId}`) return;
    saveDraft(userId, convId, { text, reply: reply ? { seq: reply.seq, author: reply.author, excerpt: reply.excerpt } : null, attachments: pending });
  }, [userId, convId, text, reply, pending]);

  // Emoji suggestions: `:` and two letters at the caret.
  const eq = mode === "write" ? emojiQuery(text, caret) : null;
  const eqKey = eq ? `${eq.from}:${eq.query}` : null;
  useEffect(() => {
    if (!eq) {
      setHits([]);
      return;
    }
    let live = true;
    void searchEmoji(eq.query).then((h) => {
      if (!live) return;
      setHits(h);
      setActive(0);
    });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [eqKey]);
  const suggesting = !!eq && hits.length > 0 && dismissed !== eqKey;

  function edit(e: Edit) {
    const ta = area.current;
    if (!ta) return;
    applyEdit(ta, e, setText);
    setCaret(ta.selectionEnd);
  }
  function format(action: FormatAction) {
    const ta = area.current;
    if (!ta) return;
    setMode("write");
    edit(applyFormat(ta.value, ta.selectionStart, ta.selectionEnd, action));
  }
  function chooseEmoji(h: EmojiHit) {
    const ta = area.current;
    if (!ta || !eq) return;
    edit({ from: eq.from, to: ta.selectionEnd, insert: h.emoji, selStart: eq.from + h.emoji.length, selEnd: eq.from + h.emoji.length });
  }
  function insertEmoji(emoji: string) {
    const ta = area.current;
    if (!ta) return;
    setPickerOpen(false);
    setMode("write");
    const at = ta.selectionStart;
    edit({ from: at, to: ta.selectionEnd, insert: emoji, selStart: at + emoji.length, selEnd: at + emoji.length });
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    const t = text.trim();
    // Send is enabled with text or at least one uploaded file (C-726), never while an upload runs.
    if ((!t && pending.length === 0) || busy || uploading.length > 0) return;
    setBusy(true);
    setSendErr(null);
    // One key per message (RFE-018): a retry of the same text and attachments reuses it, so a lost response never
    // makes a second turn; an edited message gets a fresh one (the server refuses a key reused with other content).
    const attIds = pending.map((a) => a.id);
    const sig = JSON.stringify([convId, t, [...attIds].sort(), reply?.seq ?? null]);
    if (keyRef.current?.sig !== sig) keyRef.current = { sig, key: newIdempotencyKey() };
    try {
      const out = classifySend(await chatApi.send(convId, t, attIds, keyRef.current.key, reply?.seq));
      keyRef.current = null;
      if (out.kind === "followup") void refreshFollowups();
      setText("");
      onDraftUsed?.();
      setPending([]);
      setMode("write");
      onClearReply?.();
      onSent?.();
      void qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations"] });
    } catch (err) {
      // The server took the key for other content: never reuse it.
      if (err instanceof ApiError && err.code === "idempotency_key_reused") keyRef.current = null;
      // A restored attachment the sweeper deleted (`ATTACH_PENDING_TTL`): drop the chips, keep the text.
      if (err instanceof ApiError && err.status === 422 && attIds.length > 0 && JSON.stringify(err.details ?? "").includes("/attachment_ids")) {
        setExpired(attIds.length);
        setPending([]);
        keyRef.current = null;
      } else {
        setSendErr(err);
      }
    } finally {
      setBusy(false);
    }
  }

  async function attach(list: File[]) {
    if (list.length === 0) return;
    setUploadErrs([]);
    setExpired(0);
    for (const file of list) {
      setUploading((n) => [...n, file.name]);
      try {
        const a = await uploadAttachment(convId, file);
        setPending((p) => [...p, a]);
      } catch (err) {
        setUploadErrs((m) => [...m, `Upload failed: ${file.name}: ${uploadErrorMessage(err)}`]);
      } finally {
        setUploading((n) => {
          const i = n.indexOf(file.name);
          return i < 0 ? n : [...n.slice(0, i), ...n.slice(i + 1)];
        });
      }
    }
  }
  useEffect(() => {
    if (!attachRef) return;
    attachRef.current = (files) => void attach(files);
    return () => {
      attachRef.current = null;
    };
  });

  useEffect(() => {
    if (!draftRef) return;
    draftRef.current = (t) => {
      setText(t.slice(0, 500));
      area.current?.focus();
    };
    return () => {
      draftRef.current = null;
    };
  }, [draftRef]);

  async function act(fn: () => Promise<unknown>) {
    setSendErr(null);
    try {
      await fn();
    } catch (err) {
      setSendErr(err);
    }
  }

  function onPaste(e: ClipboardEvent<HTMLTextAreaElement>) {
    const files = Array.from(e.clipboardData.files);
    if (files.length > 0) {
      e.preventDefault();
      void attach(files.map((f) => (f.type.startsWith("image/") && /^image\.\w+$/i.test(f.name) ? new File([f], pastedName(f), { type: f.type }) : f)));
      return;
    }
    // Plain text only (the textarea never takes the clipboard's HTML); a URL over a selection and spreadsheet rows
    // get Markdown.
    const ta = e.currentTarget;
    const smart = smartPaste(ta.value, ta.selectionStart, ta.selectionEnd, e.clipboardData.getData("text/plain"));
    if (smart) {
      e.preventDefault();
      edit(smart);
    }
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    const ta = e.currentTarget;
    if (suggesting) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setActive((a) => (a + (e.key === "ArrowDown" ? 1 : -1) + hits.length) % hits.length);
        return;
      }
      if ((e.key === "Enter" || e.key === "Tab") && !e.nativeEvent.isComposing) {
        e.preventDefault();
        chooseEmoji(hits[active] ?? hits[0]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(eqKey);
        return;
      }
    }
    if (e.key === "Escape" && reply) {
      e.preventDefault();
      onClearReply?.();
      return;
    }
    const mod = e.ctrlKey || e.metaKey;
    if (mod) {
      const action = shortcutAction(e);
      if (action) {
        e.preventDefault();
        format(action);
        return;
      }
    }
    if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      if (mod) {
        e.preventDefault();
        void submit();
      } else if (e.shiftKey) {
        const c = continueList(ta.value, ta.selectionStart);
        if (c && ta.selectionStart === ta.selectionEnd) {
          e.preventDefault();
          edit(c);
        }
      } else if (!inOpenFence(ta.value, ta.selectionStart)) {
        e.preventDefault();
        void submit();
      }
    }
  }

  const tooMany = sendErr instanceof ApiError && sendErr.code === "too_many_attachments";
  // (C-725) The original was redacted (or is gone) since Reply was chosen: say so in the bar, keep the text.
  const replyGone = sendErr instanceof ApiError && sendErr.code === "reply_target_invalid";
  const fenced = mode === "write" && inOpenFence(text, caret);
  const mac = isMac();
  const keysOf = (a: FormatAction) => SHORTCUTS[a]?.keys.map((k) => (mac && k === "Ctrl" ? "⌘" : k));
  const canSend = (!!text.trim() || pending.length > 0) && !busy && uploading.length === 0;
  return (
    <div className="shrink-0 space-y-2 border-t border-line bg-surface p-3 dark:bg-canvas md:px-5">
      {waiting.map((f) => {
        const after = <T,>(p: Promise<T>) => p.finally(() => void refreshFollowups());
        const failed = sendNowFailed.get(f.parent_turn_id);
        return (
          <QueuedRow
            key={f.id}
            followup={f}
            notice={
              failed === "nothing_running"
                ? "Nothing was running to move to the background."
                : failed === "unsupported"
                  ? "This agent can't move a running step to the background."
                  : null
            }
            onSteer={() => after(chatApi.steerFollowup(f.id))}
            onSendNow={() => after(chatApi.sendNow(f.parent_turn_id))}
            onSave={(t) => after(chatApi.editFollowup(f.id, t))}
            onCancel={() => after(chatApi.cancelFollowup(f.id))}
          />
        );
      })}
      {tooMany ? (
        <p role="alert" className="text-sm text-danger-ink">
          {uploadErrorMessage(sendErr)}
        </p>
      ) : sendErr && !replyGone ? (
        <ErrorBlock code={sendErr instanceof ApiError ? sendErr.code : null} message={sendErr instanceof ApiError ? sendErr.message : undefined} base={base} agentId={agentId} />
      ) : null}
      {expired > 0 ? (
        <Notice role="status">
          {expired} attached {expired === 1 ? "file" : "files"} expired. Attach {expired === 1 ? "it" : "them"} again.
        </Notice>
      ) : null}
      <form
        onSubmit={(e) => void submit(e)}
        className="rounded-card border border-line-strong bg-surface p-2 focus-within:border-accent data-[dropping]:border-accent dark:bg-canvas md:p-2.5">
        {reply ? <ReplyBar target={reply} onCancel={() => onClearReply?.()} error={replyGone ? "You can't reply to that message any more" : null} /> : null}
        {pending.length > 0 ? (
          <ul aria-label="Pending attachments" className="mb-2 flex flex-wrap gap-2">
            {pending.map((a) => (
              <ComposerChip key={a.id} attachment={a} onRemove={() => setPending((p) => p.filter((x) => x.id !== a.id))} />
            ))}
          </ul>
        ) : null}
        {uploading.length > 0 || uploadErrs.length > 0 ? (
          <div className="mb-2 flex flex-wrap gap-2">
            {uploading.map((n, i) => (
              <UploadChip key={`${n}-${i}`} name={n} />
            ))}
            {uploadErrs.map((m, i) => (
              <UploadChip key={`err-${i}`} name="" error={m} />
            ))}
          </div>
        ) : null}
        <div className="relative" id={tabPanelId(ids, "write")} role="tabpanel" aria-labelledby={tabId(ids, "write")} hidden={mode === "preview"}>
          <label htmlFor="chat-input" className="sr-only">
            Message
          </label>
          <textarea
            id="chat-input"
            ref={area}
            rows={1}
            value={text}
            placeholder={activeTurn ? "Add a follow-up…" : `Message ${agentName}…`}
            role="combobox"
            aria-expanded={suggesting}
            aria-controls={suggesting ? listId : undefined}
            aria-autocomplete="list"
            aria-activedescendant={suggesting ? optionId(listId, active) : undefined}
            onChange={(e) => {
              setText(e.target.value);
              setCaret(e.target.selectionEnd);
              onDraftUsed?.();
            }}
            onSelect={(e) => setCaret(e.currentTarget.selectionEnd)}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            className="block max-h-44 min-h-11 w-full min-w-0 resize-none rounded-control bg-transparent px-2 py-2.5 text-base text-ink [field-sizing:content] placeholder:text-muted focus-visible:shadow-none focus-visible:outline-none md:min-h-14 md:py-1.5 md:text-sm"
          />
          {suggesting ? (
            <SuggestionList
              id={listId}
              label="Emoji suggestions"
              active={active}
              onChoose={(i) => chooseEmoji(hits[i])}
              className="absolute bottom-full left-0 z-30 mb-1 w-full md:w-72"
              items={hits.map((h) => ({
                id: h.name,
                node: (
                  <>
                    <span aria-hidden="true" className="text-lg">
                      {h.emoji}
                    </span>
                    <span className="font-mono text-xs text-ink-2">
                      :<b className="font-bold text-ink">{h.name.slice(0, eq?.query.length)}</b>
                      {h.name.slice(eq?.query.length)}:
                    </span>
                  </>
                ),
              }))}
            />
          ) : null}
        </div>
        <div
          id={tabPanelId(ids, "preview")}
          role="tabpanel"
          tabIndex={mode === "preview" ? 0 : undefined}
          aria-labelledby={tabId(ids, "preview")}
          hidden={mode !== "preview"}
          onKeyDown={(e) => {
            if ((e.ctrlKey || e.metaKey) && shortcutAction(e)) {
              e.preventDefault();
              setMode("write");
              requestAnimationFrame(() => area.current?.focus());
            }
          }}
          // (chat-8) The preview grows with its content: no box of its own with a scrollbar.
          className="min-h-14 rounded-control px-2 py-1.5 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
          {mode === "preview" ? (
            text.trim() ? (
              <div className="min-w-0 max-w-full space-y-2 break-words rounded-card rounded-tr-tail bg-bubble px-3.5 py-2.5 text-bubble-ink">
                <Markdown source={text} breaks />
              </div>
            ) : (
              <p className="text-muted">Nothing to preview.</p>
            )
          ) : null}
        </div>
        <div className={fmtOn ? "block" : "hidden"} data-testid="formatting-row">
          {/* (P5-UX-1) The row and the toolbar wrap at any width: no clipped button at about 1250 px, none behind a sideways scroll at 390 px. */}
          <div className="mt-1 flex flex-wrap items-center gap-1 border-t border-line pt-1">
            <Tabs
              label="Message view"
              idBase={ids}
              value={mode}
              onValueChange={(v) => {
                setMode(v as "write" | "preview");
                if (v === "write") requestAnimationFrame(() => area.current?.focus());
              }}
              items={[
                { value: "write", label: "Write" },
                { value: "preview", label: "Preview" },
              ]}
              tabClassName="min-h-11 md:min-h-8"
              className="shrink-0"
            />
            <Toolbar label="Formatting" className="min-w-0 flex-wrap">
              {ACTIONS.map((a, i) => (
                <span key={a.action} className="contents">
                  {i > 0 && ACTIONS[i - 1].group !== a.group ? <ToolbarSeparator /> : null}
                  <ToolbarButton
                    label={a.label}
                    keys={keysOf(a.action)}
                    ariaKeys={SHORTCUTS[a.action]?.aria}
                    disabled={mode === "preview"}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => format(a.action)}>
                    <a.icon aria-hidden="true" width={18} height={18} />
                  </ToolbarButton>
                </span>
              ))}
              <ToolbarSeparator />
              <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
                <PopoverAnchor asChild>
                  <span className="inline-flex">
                    <ToolbarButton
                      label="Emoji"
                      disabled={mode === "preview"}
                      onClick={() => setPickerOpen((o) => !o)}
                      aria-haspopup="dialog"
                      aria-expanded={pickerOpen}>
                      <SmileIcon aria-hidden="true" width={18} height={18} />
                    </ToolbarButton>
                  </span>
                </PopoverAnchor>
                <PopoverContent
                  side="top"
                  align="end"
                  aria-label="Emoji picker"
                  collisionPadding={16}
                  onCloseAutoFocus={(e) => e.preventDefault()}>
                  <EmojiPicker onPick={insertEmoji} />
                </PopoverContent>
              </Popover>
            </Toolbar>
          </div>
        </div>
        <div className="mt-1 flex items-center gap-1 md:gap-1.5">
          <label className="inline-flex size-11 shrink-0 cursor-pointer items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent md:size-9">
            <PaperclipIcon width={18} height={18} />
            <span className="sr-only">Attach</span>
            <input
              type="file"
              multiple
              className="sr-only"
              aria-label="Attach files"
              onChange={(e) => {
                void attach(Array.from(e.target.files ?? []));
                e.target.value = "";
              }}
            />
          </label>
          <button
            type="button"
            aria-label="Formatting"
            aria-pressed={fmtOn}
            onClick={() => {
              saveFormattingPref(!fmtOn);
              setFmtPref(!fmtOn);
            }}
            className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-sm font-semibold text-ink-2 hover:bg-subtle hover:text-ink focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent aria-pressed:bg-subtle aria-pressed:text-ink md:size-9">
            <span aria-hidden="true">Aa</span>
          </button>
          <span className="hidden min-w-0 flex-1 truncate whitespace-nowrap px-1 text-xs text-muted md:block" data-testid="composer-hint">
            {fenced ? (
              <>
                Enter for a new line · <Kbd>{mac ? "⌘" : "Ctrl"}</Kbd> <Kbd>Enter</Kbd> to send
              </>
            ) : (
              "Enter to send · Shift+Enter for a new line"
            )}
          </span>
          <span className="flex-1 md:hidden" />
          {activeTurn ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              aria-label={stopping ? "Stopping…" : "Stop"}
              disabled={stopping}
              className="size-11 min-h-11 shrink-0 px-0 md:w-auto md:min-h-9 md:px-3"
              onClick={() => {
                setStopBusy(true);
                void act(() => chatApi.abort(activeTurn).then(() => markStopped(activeTurn))).finally(() => setStopBusy(false));
              }}>
              <StopIcon width={16} height={16} />
              <span className="hidden md:inline">{stopping ? "Stopping…" : "Stop"}</span>
            </Button>
          ) : null}
          <Button type="submit" size="lg" className="size-11 min-h-11 shrink-0 px-0" aria-label={activeTurn ? "Follow up" : "Send"} disabled={!canSend}>
            <SendIcon width={18} height={18} />
          </Button>
        </div>
      </form>
    </div>
  );
}
