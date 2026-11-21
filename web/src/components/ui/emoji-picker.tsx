"use client";

import { useEffect, useState } from "react";

import { inputClass } from "@/components/ui/field";
import { cn } from "@/lib/utils";
import { commonEmoji, searchEmoji, type EmojiHit } from "@/components/chat/composer/emoji";

/**
 * The emoji grid behind the formatting row's Emoji button (P5-CHAT-3): a search field and a grid of common emoji,
 * or the search's matches. The data loads on first open. Choosing one calls `onPick(character)` (the character,
 * never the shortcode).
 */
export function EmojiPicker({ onPick }: { onPick: (emoji: string) => void }) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<EmojiHit[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let live = true;
    const run = q.trim() ? searchEmoji(q.trim().toLowerCase().replace(/^:/, ""), 48) : commonEmoji();
    void run.then((h) => {
      if (!live) return;
      setHits(h);
      setLoading(false);
    });
    return () => {
      live = false;
    };
  }, [q]);
  return (
    <div className="space-y-2">
      <label className="sr-only" htmlFor="emoji-search">
        Search emoji
      </label>
      <input
        id="emoji-search"
        type="search"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Search emoji"
        autoComplete="off"
        className={cn(inputClass, "min-h-11 px-2 text-base md:min-h-9 md:text-sm")}
      />
      {loading ? (
        <p role="status" className="text-xs text-muted">
          Loading emoji…
        </p>
      ) : hits.length === 0 ? (
        <p role="status" className="text-xs text-muted">
          No emoji match “{q.trim()}”.
        </p>
      ) : (
        <ul aria-label="Emoji" className="grid grid-cols-8 gap-0.5">
          {hits.map((h) => (
            <li key={h.name}>
              <button
                type="button"
                aria-label={h.name.replace(/_/g, " ")}
                title={`:${h.name}:`}
                onClick={() => onPick(h.emoji)}
                className="inline-flex size-9 items-center justify-center rounded-control text-lg hover:bg-subtle focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent md:size-8">
                <span aria-hidden="true">{h.emoji}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
