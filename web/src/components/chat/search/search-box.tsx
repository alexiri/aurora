"use client";

import { useId, useMemo, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";

import { FilterChip } from "@/components/ui/filter-chip";
import { MessageIcon, SearchIcon, XIcon } from "@/components/ui/icons";
import { optionId, SuggestionList, type Suggestion } from "@/components/ui/suggestion-list";
import type { Conversation } from "@/lib/api/conversations";
import { conversationTitle, filterConversations } from "@/lib/chat/layout";
import { addChip, chipLabel, extractFilters, inTokenAtEnd, tokenize, withoutInToken, type FilterChip as Chip } from "@/lib/chat/search-syntax";
import { cn } from "@/lib/utils";

/** The filters offered under "Narrow the search" (docs/14 §Message search); `kind` hides the ones already chipped. */
const NARROW: { token: string; hint: string; kind: Chip["kind"] | null }[] = [
  { token: "from:me", hint: "messages you sent", kind: "from" },
  { token: "from:agent", hint: "messages from the agent", kind: "from" },
  { token: "from:others", hint: "everything else", kind: "from" },
  { token: "in:", hint: "one conversation", kind: "in" },
  { token: "has:file", hint: "with a file", kind: "has" },
  { token: "after:", hint: "after a day, YYYY-MM-DD", kind: null },
  { token: "before:", hint: "before a day", kind: null },
  { token: "on:", hint: "on one day", kind: null },
];

interface Option extends Suggestion {
  run: () => void;
}

/**
 * The list pane's search box (P5-CHAT-4 part B): it still filters the loaded conversations by title as you type, and
 * opens a suggestion list: "Search messages for …", matching conversations, and the filters. Finished filters become
 * chips under the box. The parent owns the text, the chips and what to do on submit.
 */
export function SearchBox({
  text,
  onTextChange,
  chips,
  onChipsChange,
  conversations,
  onSelectConversation,
  onSubmit,
  onClear,
  inputRef,
  error,
  searching,
}: {
  text: string;
  onTextChange: (t: string) => void;
  chips: Chip[];
  onChipsChange: (c: Chip[]) => void;
  /** Loaded conversations: the title matches and the `in:` list. */
  conversations: Conversation[];
  onSelectConversation: (id: string) => void;
  /** Enter on "Search messages for …": the box's current text and chips (the parent extracts a last filter word). */
  onSubmit: (text: string, chips: Chip[]) => void;
  /** Esc or Clear: empty the box and the chips and go back to the list. */
  onClear: () => void;
  inputRef: RefObject<HTMLInputElement | null>;
  /** A field error from the API (422 `search_query_invalid`). */
  error?: string | null;
  /** A message search is showing: Clear stays available even with an empty box. */
  searching: boolean;
}) {
  const listId = useId();
  const [focused, setFocused] = useState(false);
  const [closed, setClosed] = useState(false);
  // `null` = the default row: the first one when it is a search or an `in:` match, none for a bare list of filters.
  const [active, setActive] = useState<number | null>(null);

  const options = useMemo<Option[]>(() => {
    const out: Option[] = [];
    const inTok = inTokenAtEnd(text);
    if (inTok !== null) {
      for (const c of filterConversations(conversations, inTok).slice(0, 6)) {
        const title = conversationTitle(c);
        out.push({
          id: `in-${c.id}`,
          node: (
            <>
              <MessageIcon aria-hidden="true" width={14} height={14} className="shrink-0 text-muted" />
              <span className="truncate">{title}</span>
            </>
          ),
          run: () => {
            onChipsChange(addChip(chips, { kind: "in", value: c.id, label: title }));
            onTextChange(withoutInToken(text));
          },
        });
      }
      return out;
    }
    const q = text.trim();
    if (q) {
      out.push({
        id: "search",
        node: (
          <>
            <SearchIcon aria-hidden="true" width={14} height={14} className="shrink-0 text-muted" />
            <span className="truncate">Search messages for “{q}”</span>
          </>
        ),
        run: () => onSubmit(text, chips),
      });
      for (const c of filterConversations(conversations, q).slice(0, 3)) {
        out.push({
          id: `conv-${c.id}`,
          node: (
            <>
              <MessageIcon aria-hidden="true" width={14} height={14} className="shrink-0 text-muted" />
              <span className="min-w-0 flex-1 truncate">{conversationTitle(c)}</span>
              <span className="shrink-0 text-xs text-muted">Conversation</span>
            </>
          ),
          run: () => onSelectConversation(c.id),
        });
      }
    }
    const last = tokenize(text).at(-1);
    const partial = last && !last.complete && last.end === text.length ? last.text.toLowerCase() : "";
    for (const n of NARROW) {
      if (n.kind && chips.some((c) => c.kind === n.kind)) continue;
      if (partial && !n.token.startsWith(partial)) continue;
      out.push({
        id: `narrow-${n.token}`,
        node: (
          <>
            <span className="font-mono text-xs text-ink">{n.token}</span>
            <span className="min-w-0 flex-1 truncate text-xs text-muted">{n.hint}</span>
          </>
        ),
        run: () => {
          const head = partial ? text.slice(0, text.length - partial.length) : text;
          const sep = head && !/\s$/.test(head) ? " " : "";
          change(`${head}${sep}${n.token}${n.token.endsWith(":") ? "" : " "}`);
        },
      });
    }
    return out;
    // `change` is stable enough for this memo: it only reads the props listed here
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, chips, conversations]);

  const open = focused && !closed && options.length > 0;
  const first = options[0]?.id === "search" || options[0]?.id.startsWith("in-") ? 0 : -1;
  const index = active === null ? first : Math.min(active, options.length - 1);

  /** Typing: a finished filter word moves to a chip. */
  function change(value: string) {
    const r = extractFilters(value, chips);
    if (r.chips !== chips) onChipsChange(r.chips);
    onTextChange(r.rest);
    setActive(null);
    setClosed(false);
  }

  /** Runs a suggestion and closes the list (an option that edits the text opens it again through `change`). */
  function choose(i: number) {
    setClosed(true);
    options[i]?.run();
  }

  function key(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Escape") {
      if (open) {
        e.preventDefault();
        setClosed(true);
        return;
      }
      if (text || chips.length > 0 || searching) {
        e.preventDefault();
        onClear();
      }
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (options.length === 0) return;
      e.preventDefault();
      setClosed(false);
      const n = options.length;
      setActive(e.key === "ArrowDown" ? (index + 1) % n : (index <= 0 ? n : index) - 1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (open && index >= 0) choose(index);
      else if (text.trim()) {
        setClosed(true);
        onSubmit(text, chips);
      }
    } else if (e.key === "Backspace" && text === "" && chips.length > 0) {
      onChipsChange(chips.slice(0, -1));
    }
  }

  const errId = `${listId}-err`;
  const show: ReactNode = open ? (
    <div className="absolute inset-x-0 top-full z-20 mt-1 rounded-card border border-line bg-surface shadow-pop">
      <SuggestionList id={listId} label="Search suggestions" items={options} active={index} onChoose={choose} className="rounded-none border-0 shadow-none" />
      <p className="border-t border-line px-3 py-2 text-xs text-muted">
        <span className="font-mono">&quot;exact phrase&quot;</span>, <span className="font-mono">-word</span> to exclude, <span className="font-mono">OR</span> between words
      </p>
    </div>
  ) : null;

  return (
    <div className="space-y-2">
      <div className="relative">
        <SearchIcon aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted" />
        <input
          ref={inputRef}
          type="search"
          role="combobox"
          aria-label="Search conversations and messages"
          aria-expanded={open}
          aria-controls={open ? listId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={open && index >= 0 ? optionId(listId, index) : undefined}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errId : undefined}
          placeholder="Search"
          autoComplete="off"
          value={text}
          onChange={(e) => change(e.target.value)}
          onKeyDown={key}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          className={cn(
            "min-h-11 w-full rounded-control border border-line-strong bg-surface pl-9 pr-11 text-sm text-ink placeholder:text-muted md:min-h-9 dark:bg-canvas",
            "[&::-webkit-search-cancel-button]:hidden",
            error && "border-danger-ink",
          )}
        />
        {text || chips.length > 0 || searching ? (
          <button
            type="button"
            aria-label="Clear search"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              onClear();
              inputRef.current?.focus();
            }}
            className="absolute right-0 top-1/2 inline-flex size-11 -translate-y-1/2 items-center justify-center rounded-control text-muted hover:text-ink md:size-9">
            <XIcon aria-hidden="true" width={14} height={14} />
          </button>
        ) : null}
        {show}
      </div>
      {error ? (
        <p id={errId} role="alert" className="text-xs text-danger-ink">
          {error}
        </p>
      ) : null}
      {chips.length > 0 ? (
        <div role="group" aria-label="Search filters" className="flex flex-wrap gap-1.5">
          {chips.map((c) => (
            <FilterChip key={c.kind} label={chipLabel(c)} onRemove={() => onChipsChange(chips.filter((x) => x !== c))} />
          ))}
        </div>
      ) : null}
    </div>
  );
}
