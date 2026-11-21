/**
 * (P5-REFINE-20) The one chip beside an agent's name, in words a person uses (never the API's `kind` value): "Came with
 * this workspace" for the default agent, else "Bring your own" for one that runs on the person's own computer, else none.
 * At most one chip, so a header and a list row never crowd.
 */
export function agentChip(agent: { kind: string; is_default?: boolean | null }): { text: string; tone: "accent" | "neutral" } | null {
  if (agent.is_default) return { text: "Came with this workspace", tone: "accent" };
  if (agent.kind === "byo") return { text: "Bring your own", tone: "neutral" };
  return null;
}
