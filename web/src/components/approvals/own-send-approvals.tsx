"use client";

// Posts agents propose to make as the signed-in person (phase 5, P4-UTOK, D-283, C-856, docs/14 §Account): the full text
// and the decision controls, shown to the person whose token it is and to nobody else. Hidden while there are none.
import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/api/errors";
import { isAlreadyDecided } from "@/lib/api/action-approvals";
import { useDecideOwnSend, useOwnSendApprovals } from "@/lib/api/user-tokens";

export function OwnSendApprovals() {
  const q = useOwnSendApprovals("pending");
  const decide = useDecideOwnSend();
  const [messages, setMessages] = useState<Record<string, string>>({});
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const items = q.data ?? [];
  if (items.length === 0) return null;
  return (
    <section aria-label="Posts as you" className="space-y-3">
      <h2 className="text-lg font-semibold text-ink">Posts as you</h2>
      <p className="text-sm text-ink-2">
        An agent asks to post this under your own chat account. Check the exact text: nothing is posted unless you allow it. A line
        saying an agent wrote it is added at the end.
      </p>
      <ul className="space-y-2">
        {items.map((a) => (
          <li key={a.id} className="space-y-3 rounded-card border border-line bg-surface p-4 text-ink shadow-card sm:p-5">
            <p className="whitespace-pre-wrap font-display text-base font-semibold">{a.summary}</p>
            {/* The text is the agent's: a plain React text node, never HTML. */}
            <blockquote className="max-h-72 overflow-y-auto whitespace-pre-wrap break-words rounded-control border border-line bg-canvas p-3 text-sm text-ink">
              {a.text}
            </blockquote>
            <p className="text-xs text-muted">Expires {fmtDate(a.expires_at)}</p>
            {messages[a.id] ? (
              <p role="alert" className="rounded-control border border-pending-line bg-pending-panel p-3 text-sm text-pending-ink">
                {messages[a.id]}
              </p>
            ) : null}
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                size="lg"
                variant="outline"
                disabled={decide.isPending && decide.variables?.id === a.id}
                onClick={() => decideOne(a.id, "deny")}
              >
                Don&apos;t post
              </Button>
              <Button
                size="lg"
                variant="approve"
                disabled={decide.isPending && decide.variables?.id === a.id}
                onClick={() => decideOne(a.id, "approve")}
              >
                Post as me
              </Button>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );

  function decideOne(id: string, decision: "approve" | "deny") {
    setMessages((m) => ({ ...m, [id]: "" }));
    decide.mutate(
      { id, decision },
      {
        onError: (err) =>
          setMessages((m) => ({ ...m, [id]: isAlreadyDecided(err) ? "Already decided." : errorMessage(err) })),
      },
    );
  }
}
