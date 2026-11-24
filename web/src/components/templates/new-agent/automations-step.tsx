"use client";

import { Notice } from "@/components/ui/notice";
import { SwitchField } from "@/components/ui/switch";

import { cadenceWords, whenWords } from "@/lib/cron";

// P6-TPL-4 Part B1, C-1116 (the Automations step): each schedule with its cadence in words, its prompt, the time zone
// (the workspace's) and a switch; webhooks are listed as created paused. What is left switched on is created.

/** One automation of the template's content (C-1104's declaration, as stored). */
export interface TemplateAutomation {
  kind: string;
  name: string;
  prompt?: string | null;
  cron?: string | null;
  instructions?: string | null;
  verification?: string | null;
  /** The template's own default; a schedule left out of the create request is created with it. */
  enabled?: boolean | null;
}

export interface AutomationState {
  enabled: boolean;
}

const VERIFICATION_WORDS: Record<string, string> = {
  signed: "Verified with a signature.",
  password: "Verified with a password.",
  none: "No verification.",
};

export function automationEnabled(a: TemplateAutomation, state: Record<string, AutomationState>): boolean {
  return state[a.name]?.enabled ?? a.enabled !== false;
}

export function AutomationsStep({
  automations,
  state,
  onChange,
  timezone,
}: {
  automations: TemplateAutomation[];
  state: Record<string, AutomationState>;
  onChange: (name: string, next: AutomationState) => void;
  /** The workspace's time zone (C-1116): said with each schedule's cadence. */
  timezone?: string | null;
}) {
  const schedules = automations.filter((a) => a.kind === "schedule");
  const webhooks = automations.filter((a) => a.kind !== "schedule");
  if (automations.length === 0) return <p className="text-sm text-ink-2">The agent runs nothing on its own.</p>;
  return (
    <div className="space-y-3">
      {schedules.map((a) => (
        <div key={a.name} className="space-y-1 rounded-card border border-line bg-surface p-4 text-sm">
          <SwitchField
            id={`automation-${a.name}`}
            label={a.name}
            checked={automationEnabled(a, state)}
            onChange={(checked) => onChange(a.name, { enabled: checked })}
          />
          {a.prompt ? <p className="whitespace-pre-wrap text-ink-2 [overflow-wrap:anywhere]">{a.prompt}</p> : null}
          <p className="text-xs text-muted">
            {whenWords(a.cron, timezone)}
            {a.cron && !cadenceWords(a.cron) ? <span className="font-mono"> {a.cron}</span> : null}
          </p>
        </div>
      ))}
      {webhooks.map((a) => (
        <div key={a.name} className="space-y-1 rounded-card border border-line bg-surface p-4 text-sm">
          <p className="font-medium text-ink">
            {a.name} <span className="font-normal text-muted">Webhook</span>
          </p>
          <p className="text-ink-2">Created paused. {a.instructions ?? ""} {a.verification ? VERIFICATION_WORDS[a.verification] ?? "" : ""}</p>
        </div>
      ))}
      <Notice tone="neutral">
        <p>Webhooks are created paused; the agent turns one on when you give it the address.</p>
      </Notice>
    </div>
  );
}
