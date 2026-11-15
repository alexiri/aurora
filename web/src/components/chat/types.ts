/** Who the agent is, for the small avatar next to its text and the empty-state card (display only). */
export interface AgentIdent {
  id: string;
  name: string;
  avatarUrl?: string | null;
  /** The agent's brief, shown in the empty conversation when set. */
  brief?: string | null;
  /** The agent is frozen (C-205): a failed start cannot be retried (C-615 hides **Retry now**). */
  frozen?: boolean;
}
