import { Badge } from "@/components/ui/badge";
import { tokenStates, type TokenFlags, type TokenState } from "@/lib/api/agent-tokens";

const LABEL: Record<TokenState, { text: string; tone: "danger" | "pending" | "neutral"; title: string }> = {
  expired: {
    text: "Token expired",
    tone: "danger",
    title: "The gateway refuses this token. Regenerate it on the Connect tab.",
  },
  expiring: {
    text: "Token expiring",
    tone: "pending",
    title: "This token expires within 14 days. Regenerate it before then.",
  },
  stale: {
    text: "Token unused",
    tone: "neutral",
    title: "This token has not been used for 30 days. Revoke it by regenerating, or delete the agent.",
  },
};

/** (RFE-013) Stale / expiring / expired badges of an agent token; nothing for a healthy one. */
export function TokenBadges({ token, now }: { token: TokenFlags | null | undefined; now?: number }) {
  const states = tokenStates(token, now);
  if (states.length === 0) return null;
  return (
    <>
      {states.map((s) => (
        <Badge key={s} tone={LABEL[s].tone} title={LABEL[s].title}>
          {LABEL[s].text}
        </Badge>
      ))}
    </>
  );
}
