/**
 * (P5-MEMORY-4) **Save it yourself** on a refused memory card: what the Memory page's form starts with. It is handed
 * over in memory, for the one client-side navigation that follows the click: the text of a memory never goes into the
 * address, browser storage or a log.
 */
export type MemoryDraft =
  /** The new-memory form. `key` is null when it is taken: the form then makes one from the title. */
  | { kind: "new"; key: string | null; title: string; content: string }
  /** An existing memory in edit mode, with `content` (its text and what the agent wanted to add) not saved yet. */
  | { kind: "edit"; key: string; content: string };

let pending: { agentId: string; draft: MemoryDraft } | null = null;

export function setMemoryDraft(agentId: string, draft: MemoryDraft): void {
  pending = { agentId, draft };
}

/** The draft waiting for this agent's Memory page, if any. Reading does not drop it (a render may run twice). */
export function peekMemoryDraft(agentId: string): MemoryDraft | null {
  return pending?.agentId === agentId ? pending.draft : null;
}

/** Called once the page holds the draft in its own state. */
export function clearMemoryDraft(): void {
  pending = null;
}
