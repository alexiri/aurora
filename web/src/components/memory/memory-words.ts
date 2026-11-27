/**
 * (P5-MEMORY-2, D-497) The sentences that say where a memory came from and when the agent used it. A conversation is
 * named only when the API returned its reference with a title; otherwise it is "in a conversation" — never a guess, and
 * never a link.
 */
import type { ConversationRef, Memory, MemoryActor } from "@/lib/api/memory";
import { formatAge, formatShortDay } from "@/lib/format-date";
import { personName } from "@/lib/workspace-name";

export function conversationWords(conversation: ConversationRef | null | undefined): string {
  const title = conversation?.title?.trim();
  return title ? `in “${title}”` : "in a conversation";
}

/** The signed-in person added it themselves (also through their own API key). */
export function addedByMe(actor: MemoryActor, me: string | null | undefined): boolean {
  return Boolean(me) && (actor.kind === "person" || actor.kind === "api_key") && actor.person?.id === me;
}

/**
 * "Added by you", "Added by <Person>", "Added through <key name>", "Learned by <Agent> in “<conversation>”",
 * "From the template <name>". `asYou: false` names the signed-in person (the reading view's meta line).
 */
export function sourceWords(actor: MemoryActor, agentName: string, me: string | null | undefined, asYou = true): string {
  switch (actor.kind) {
    case "agent":
      return `Learned by ${actor.agent?.name || agentName} ${conversationWords(actor.conversation)}`;
    case "template":
      return actor.template ? `From the template ${actor.template.name}` : "From a template";
    case "import":
      return "Imported";
    case "person":
    case "api_key": {
      if (addedByMe(actor, me)) return asYou ? "Added by you" : `Added by ${personName(actor.person?.display_name)}`;
      if (actor.kind === "api_key" && actor.api_key?.name) return `Added through ${actor.api_key.name}`;
      if (actor.person) return `Added by ${personName(actor.person.display_name)}`;
      return actor.kind === "api_key" ? "Added through an API key" : "Added";
    }
    default:
      return "Added";
  }
}

/** The row's third line: "Added by you · 2 days ago". */
export function sourceLine(m: Memory, agentName: string, me: string | null | undefined, now: number): string {
  return `${sourceWords(m.added_by, agentName, me)} · ${formatAge(m.created_at, now)}`;
}

export function changedWords(revision: number): string | null {
  const times = revision - 1;
  if (times < 1) return null;
  if (times === 1) return "Changed once";
  if (times === 2) return "Changed twice";
  return `Changed ${times} times`;
}

export function lastUsedWords(m: Memory, agentName: string, now: number): string {
  if (!m.last_used) return `${agentName} hasn't used it yet`;
  return `${agentName} last used it ${conversationWords(m.last_used.conversation)} ${formatAge(m.last_used.at, now)}`;
}

/** "Added by Bob Builder on 25 Nov · Changed twice · Daneel last used it in “Weekly report” yesterday". */
export function metaLine(m: Memory, agentName: string, me: string | null | undefined, now: number): string {
  // The meta line always names the person, "Added by Bob Builder", where the list says "Added by you".
  const added = sourceWords(m.added_by, agentName, me, false);
  return [`${added} on ${formatShortDay(m.created_at, now)}`, changedWords(m.revision), lastUsedWords(m, agentName, now)]
    .filter(Boolean)
    .join(" · ");
}
