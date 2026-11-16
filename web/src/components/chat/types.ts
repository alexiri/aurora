/** Who the agent is, for the small avatar next to its text and the empty-state card (display only). */
export interface AgentIdent {
  id: string;
  name: string;
  avatarUrl?: string | null;
  /** The agent's brief (its instructions); the About part of the status rail shows it, never the empty conversation. */
  brief?: string | null;
  /** The agent's handle, shown under "Say hello to <Agent>" in the empty conversation. */
  handle?: string;
  /** The agent is frozen (C-205): a failed start cannot be retried (C-615 hides **Retry now**). */
  frozen?: boolean;
}
