/** Who the agent is, for the small avatar next to its text and the empty-state card (display only). */
export interface AgentIdent {
  id: string;
  name: string;
  avatarUrl?: string | null;
  /** The agent's brief, shown in the empty conversation when set. */
  brief?: string | null;
}
