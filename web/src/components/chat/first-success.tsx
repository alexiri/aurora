"use client";

import { useMemo } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useApp } from "@/lib/api/apps";
import type { Conversation, TranscriptEvent } from "@/lib/api/conversations";
import { useMeOnboarding, usePatchOnboarding } from "@/lib/api/onboarding";
import { useSchedules, useScheduleRuns } from "@/lib/api/schedules";
import { originRunCard, pendingFirstSuccesses, runSentence, type FirstSuccess } from "@/lib/chat/first-success";

/**
 * (P5-REFINE-11 item 9, rec 10) The first successes, as `Notice` cards in the thread under what triggered them: the first
 * answer, the first app, the first approval, the first schedule run. Each has a **Got it** that remembers the dismissal on the
 * server (`notices_seen`), so it never returns, also after a reload. They say what happened, are never an approval (no "Live"
 * pill) and are not samples.
 */
export function FirstSuccessCards({
  events,
  conversation,
  agentName,
  wsId,
  agentId,
  onOpenConversation,
  onSeeWhatItDid,
  onTry,
}: {
  events: readonly TranscriptEvent[];
  conversation: Pick<Conversation, "kind" | "title" | "id"> | null;
  agentName: string;
  wsId: string;
  /** The agent whose schedules the first-run card looks at. */
  agentId?: string;
  /** **Open the run**: opens a conversation (the run's thread). */
  onOpenConversation?: (id: string) => void;
  /** "See what it did": opens the rail's "What <Agent> did". */
  onSeeWhatItDid?: () => void;
  /** Puts a suggested message in the composer (never sends it). */
  onTry?: (text: string) => void;
}) {
  const me = useMeOnboarding();
  const seen = me.data?.notices_seen;
  // (P5-REFINE-7, C-1074) A schedule made in this chat runs in threads of its own: the chat gets the card with **Open the run**.
  const wantRun = !!agentId && !!seen && !seen.includes("first-run") && conversation?.kind === "direct";
  const schedules = useSchedules(agentId ?? "", wantRun);
  const origin = wantRun ? schedules.data?.items.find((s) => s.origin_conversation_id === conversation?.id) : undefined;
  const runs = useScheduleRuns(agentId ?? "", origin?.id ?? "", !!origin);
  const originCard = originRunCard(origin, runs.data?.pages[0]?.items[0], new Date());
  const cards = useMemo(() => {
    const own = seen ? pendingFirstSuccesses(events, seen, conversation, new Date()) : [];
    return originCard ? [...own, originCard] : own;
  }, [events, seen, conversation, originCard]);
  if (cards.length === 0) return null;
  return (
    <div className="space-y-2" data-testid="first-successes">
      {cards.map((c) => (
        <Card key={c.key} card={c} agentName={agentName} wsId={wsId} events={events} onSeeWhatItDid={onSeeWhatItDid} onTry={onTry} onOpenRun={onOpenConversation} />
      ))}
    </div>
  );
}

function Card({
  card,
  agentName,
  wsId,
  onSeeWhatItDid,
  onTry,
  onOpenRun,
}: {
  card: FirstSuccess;
  agentName: string;
  wsId: string;
  events: readonly TranscriptEvent[];
  onOpenRun?: (id: string) => void;
  onSeeWhatItDid?: () => void;
  onTry?: (text: string) => void;
}) {
  const patch = usePatchOnboarding();
  const app = useApp(card.key === "first-app" ? card.appId || undefined : undefined, wsId);
  const prompt = card.key === "first-app" ? (app.data?.first_prompt ?? null) : null;
  const dismiss = () => patch.mutate({ notice_seen: card.key });
  let text: string;
  let action: React.ReactNode = null;
  switch (card.key) {
    case "first-answer":
      // "changed nothing outside this chat" is only said when the turn used no tools.
      text = card.tools === 0 ? `${agentName} answered. It thought with the Company key and changed nothing outside this chat.` : `${agentName} answered and used ${card.tools} ${card.tools === 1 ? "tool" : "tools"}.`;
      action = onSeeWhatItDid ? (
        <Button type="button" variant="outline" size="sm" className="min-h-11 md:min-h-9" onClick={onSeeWhatItDid}>
          See what it did
        </Button>
      ) : null;
      break;
    case "first-app":
      text = `${agentName} can now read ${card.appName || "the app"}.${prompt ? ` Try: “${prompt}”` : ""}`;
      action =
        prompt && onTry ? (
          <Button type="button" variant="outline" size="sm" className="min-h-11 md:min-h-9" onClick={() => onTry(prompt)}>
            Put it in the message box
          </Button>
        ) : null;
      break;
    case "first-approval":
      text = `You allowed it once. Next time ${agentName} will ask again; choose Always allow when you trust it.`;
      break;
    case "first-run":
      text = runSentence(card.title, card.at);
      {
        const run = card.conversationId;
        action =
          run && onOpenRun ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="min-h-11 md:min-h-9"
              onClick={() => {
                // Opening the run is seeing it: the card is done.
                patch.mutate({ notice_seen: card.key });
                onOpenRun(run);
              }}>
              Open the run
            </Button>
          ) : null;
      }
      break;
  }
  return (
    <Notice tone="accent" role="status" className="flex flex-wrap items-center gap-2" data-testid={`first-success-${card.key}`}>
      <p className="min-w-0 flex-1">{text}</p>
      {action}
      <Button type="button" variant="outline" size="sm" className="min-h-11 md:min-h-9" disabled={patch.isPending} onClick={dismiss}>
        Got it
      </Button>
    </Notice>
  );
}
