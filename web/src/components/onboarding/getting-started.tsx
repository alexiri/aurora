"use client";

import { useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { useState, type ReactNode } from "react";

import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { GiveAccessPanel } from "@/components/credentials/give-access";
import { LlmKeyForm } from "@/components/credentials/llm-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Field, inputClass } from "@/components/ui/field";
import { CheckCircleIcon } from "@/components/ui/icons";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { useAccess } from "@/lib/api/account";
import { usePatchAgent } from "@/lib/api/agents";
import { grantKeys } from "@/lib/api/credential-grants";
import { useModels } from "@/lib/api/conversations";
import { errorMessage } from "@/lib/api/errors";
import { useMeOnboarding, usePatchOnboarding } from "@/lib/api/onboarding";
import { useAgents, type Agent } from "@/lib/api/queries";
import { HELLO_MESSAGE, type GuideStep, type GuideStepId } from "@/lib/onboarding/guide";
import { useGuide } from "@/lib/onboarding/use-guide";
import { cn } from "@/lib/utils";

import { REASON_COPY } from "@/components/agents/model-readiness";

/** Bar widths by steps done (the CSP blocks inline styles, so widths come from a fixed class table). */
const WIDTH = ["w-0", "w-1/4", "w-2/4", "w-3/4", "w-full"];

const TITLES: Record<GuideStepId, string> = {
  key: "Add an LLM key",
  access: "Give {agent} access to it",
  model: "Choose a model",
  hello: "Say hello",
};

const stepAnchor = (id: GuideStepId) => `guide-${id}`;

export interface GuideProps {
  org: string;
  ws: string;
  wsId: string;
  orgId: string;
  /** Workspace owner (an org owner or admin counts). */
  isOwner: boolean;
  /** Welcome page (the full guide and the optional list) or the Overview slot (one line once done or dismissed). */
  variant: "welcome" | "overview";
  /** The step a link from an error asked for. */
  focus?: GuideStepId | null;
}

/** Workspace owners' names for a member who cannot do a step. `can_own` is needed to read them. */
function useOwnerNames(wsId: string, enabled: boolean): string[] {
  const access = useAccess(wsId, enabled);
  if (!enabled) return [];
  return (access.data?.items ?? []).filter((b) => b.role === "owner").map((b) => b.display_name).slice(0, 3);
}

function ownerAsk(names: string[], what: string): string {
  return names.length > 0
    ? `Ask a workspace owner (${names.join(", ")}) to ${what}.`
    : `Ask a workspace owner to ${what}.`;
}

/**
 * "Get {agent} talking" (D-141): the numbered guide. Steps tick from real state. Done steps collapse to one line that
 * says what provides them; a member who cannot do a step sees who can. On Overview it stays until done or dismissed.
 */
export function GettingStarted(props: GuideProps) {
  const agents = useAgents(props.wsId);
  const onboarding = useMeOnboarding();
  const patch = usePatchOnboarding();
  const agent: Agent | undefined = agents.data?.items.find((a) => a.is_default) ?? agents.data?.items[0];
  const state = useGuide({ wsId: props.wsId, orgId: props.orgId, agentId: agent?.id, isOwner: props.isOwner });
  const ownerNames = useOwnerNames(props.wsId, !props.isOwner);
  const [open, setOpen] = useState(false);
  const base = `/o/${props.org}/w/${props.ws}`;
  const dismissed = onboarding.data?.guide_dismissed?.includes(props.wsId) === true;

  if (agents.isPending) return <Loading />;
  if (agents.isError) return <ErrorState error={agents.error} onRetry={() => agents.refetch()} />;
  if (!agent) {
    return (
      <Card className="flex flex-wrap items-center justify-between gap-3">
        <p className="min-w-0 text-sm text-ink-2">
          An agent is the assistant that does the work. This workspace has none yet.
        </p>
        {props.isOwner ? <CreateAgentDialog wsId={props.wsId} /> : <p className="text-sm text-ink-2">{ownerAsk(ownerNames, "create one")}</p>}
      </Card>
    );
  }
  if (state.loading || !state.guide) return <Loading />;
  const guide = state.guide;

  // Overview: once done or dismissed, one line.
  if (props.variant === "overview" && (guide.complete || dismissed) && !open) {
    return (
      <Card className="flex flex-wrap items-center justify-between gap-3 py-3 sm:py-3">
        <p className="text-sm font-semibold text-ink">
          Getting started — {guide.done}/{guide.total}
          {guide.complete ? <span className="sr-only"> (done)</span> : null}
        </p>
        <Button variant="ghost" size="sm" onClick={() => setOpen(true)}>
          {guide.complete ? "Show again" : "Show"}
        </Button>
      </Card>
    );
  }

  const firstTodo = guide.steps.find((s) => s.state === "todo");
  const expanded = props.focus ?? firstTodo?.id ?? null;
  const title = `Get ${agent.name} talking`;

  return (
    <Card aria-labelledby="guide-h" role="region">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h2 id="guide-h" className="font-display text-base font-semibold text-ink">
          {title}
        </h2>
        <span className="text-sm font-semibold text-ink">
          {guide.done} of {guide.total} done
        </span>
      </div>
      <div
        role="progressbar"
        aria-label="Getting started progress"
        aria-valuemin={0}
        aria-valuemax={guide.total}
        aria-valuenow={guide.done}
        className="mb-4 h-2 overflow-hidden rounded-full bg-subtle"
      >
        <div className={cn("h-full rounded-full bg-accent", WIDTH[Math.min(guide.done, WIDTH.length - 1)])} />
      </div>
      <ol className="space-y-3">
        {guide.steps
          .filter((s) => s.state !== "skipped")
          .map((s, i) => (
            <StepRow key={s.id} step={s} index={i + 1} agent={agent} expanded={expanded === s.id}>
              <StepBody step={s} agent={agent} {...props} base={base} ownerNames={ownerNames} keysCount={state.keys.length} />
            </StepRow>
          ))}
      </ol>
      {props.variant === "overview" && !dismissed && !guide.complete ? (
        <div className="mt-4 flex justify-end">
          <Button
            variant="ghost"
            size="sm"
            // `guide_dismissed` REPLACES the whole list (PatchOnboarding): never send it before the
            // current list is known, and build it from a fresh read, not from a possibly stale render.
            disabled={patch.isPending || !onboarding.data || onboarding.isFetching}
            onClick={async () => {
              const fresh = (await onboarding.refetch()).data;
              if (!fresh) return;
              patch.mutate({ guide_dismissed: [...new Set([...(fresh.guide_dismissed ?? []), props.wsId])] });
            }}
          >
            Dismiss
          </Button>
        </div>
      ) : null}
      {props.variant === "overview" && open ? (
        <div className="mt-4 flex justify-end">
          <Button variant="ghost" size="sm" onClick={() => setOpen(false)}>
            Hide
          </Button>
        </div>
      ) : null}
    </Card>
  );
}

function StepRow({ step, index, agent, expanded, children }: { step: GuideStep; index: number; agent: Agent; expanded: boolean; children: ReactNode }) {
  const done = step.state === "done";
  const title = TITLES[step.id].replace("{agent}", agent.name);
  return (
    <li id={stepAnchor(step.id)} data-state={step.state} className="scroll-mt-20">
      <div className="flex items-start gap-3">
        <span
          aria-hidden="true"
          className={cn(
            "mt-0.5 inline-flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
            done ? "bg-accent text-on-accent" : "bg-subtle text-ink-2",
          )}
        >
          {done ? <CheckCircleIcon className="size-4" /> : index}
        </span>
        <div className="min-w-0 flex-1">
          <p className="flex flex-wrap items-center gap-x-2 text-sm">
            <span className={cn("font-semibold", done ? "text-muted" : "text-ink")}>{title}</span>
            {done ? <Badge tone="accent">Done</Badge> : null}
            {step.state === "owner" ? <Badge>Needs an owner</Badge> : null}
          </p>
          {done && step.provider ? <p className="mt-0.5 text-sm text-ink-2">{step.provider}</p> : null}
          {!done && (expanded || step.state === "owner" || step.state === "waiting") ? <div className="mt-2 space-y-3 text-sm text-ink-2">{children}</div> : null}
        </div>
      </div>
    </li>
  );
}

function StepBody({
  step,
  agent,
  wsId,
  orgId,
  isOwner,
  base,
  ownerNames,
  keysCount,
}: GuideProps & { step: GuideStep; agent: Agent; base: string; ownerNames: string[]; keysCount: number }) {
  // "Not now" on the access panel: put the panel away for this visit; the step stays open.
  const [accessDeferred, setAccessDeferred] = useState(false);
  if (step.state === "done") return null;
  if (step.state === "owner" && step.id !== "hello") {
    return <p>{ownerAsk(ownerNames, step.id === "key" ? "add an LLM key" : `give ${agent.name} access to it`)}</p>;
  }
  switch (step.id) {
    case "key":
      return step.state === "waiting" ? (
        <p>Checking what {agent.name} can use…</p>
      ) : (
        <KeyStep step={step} wsId={wsId} orgId={orgId} agent={agent} base={base} keysCount={keysCount} />
      );
    case "access":
      if (step.state === "waiting") return <p>Comes after the key.</p>;
      if (step.keyToGrant && accessDeferred) {
        return (
          <div className="flex flex-wrap items-center gap-2">
            <p>{agent.name} cannot use {step.keyToGrant.name} until it has access.</p>
            <Button size="sm" variant="outline" onClick={() => setAccessDeferred(false)}>
              Give access
            </Button>
          </div>
        );
      }
      return step.keyToGrant ? (
        <div className="space-y-2">
          <p>
            An agent can only use a key it has been given. Give <strong>{agent.name}</strong> access to <strong>{step.keyToGrant.name}</strong>.
          </p>
          <GiveAccessPanel
            heading={false}
            dismissLabel="Not now"
            onDismiss={() => setAccessDeferred(true)}
            subject={{
              target: { kind: "secret", id: step.keyToGrant.id },
              scope: { kind: step.keyToGrant.scope, id: step.keyToGrant.scope === "workspace" ? wsId : orgId },
              orgId,
              preselect: [agent.id],
            }}
          />
        </div>
      ) : null;
    case "model":
      return <ModelStep agent={agent} wsId={wsId} />;
    case "hello":
      return (
        <div className="space-y-2">
          {step.state === "waiting" ? <p>Comes after the steps above.</p> : <p>Open the chat and send a first message. The answer proves everything works.</p>}
          {step.state === "waiting" && isOwner ? (
            // Not a link: an `aria-disabled` anchor is still focusable and navigable.
            <Button size="sm" disabled>
              Say hello to {agent.name}
            </Button>
          ) : isOwner || step.state === "todo" ? (
            <Link
              href={`${base}/agents/${agent.handle}?tab=chat&draft=${encodeURIComponent(HELLO_MESSAGE)}`}
              // `text-on-primary!`: the global `a { color: inherit }` would otherwise win over the utility.
              className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}
            >
              Say hello to {agent.name}
            </Link>
          ) : null}
        </div>
      );
  }
}

function KeyStep({ step, wsId, orgId, agent, base, keysCount }: { step: GuideStep; wsId: string; orgId: string; agent: Agent; base: string; keysCount: number }) {
  const reason = step.problem ? (REASON_COPY[step.problem] ?? { text: step.problem, fix: "" }) : null;
  return (
    <div className="space-y-3">
      {reason ? (
        <Notice tone="danger" role="status">
          {agent.name} has a key, but it cannot use it. {reason.text} {reason.fix}{" "}
          <Link href={`${base}/credentials?tab=llm`} className="font-semibold underline">
            Open LLM keys
          </Link>
        </Notice>
      ) : (
        <>
          <p>
            An LLM key is the password to the AI service that does {agent.name}&apos;s thinking. Get one from your AI provider or
            from whoever runs it for your team. Aurora stores it sealed and never shows it again.
          </p>
          {keysCount === 0 ? null : <p>Keys you already have are listed under Credentials.</p>}
          <LlmKeyForm scope={{ kind: "workspace", id: wsId }} orgId={orgId} preselect={[agent.id]} />
        </>
      )}
    </div>
  );
}

/** The model step (only shown when no model resolves): pick one of the models the usable keys serve. */
function ModelStep({ agent, wsId }: { agent: Agent; wsId: string }) {
  const qc = useQueryClient();
  const models = useModels(agent.id, true);
  const patch = usePatchAgent(wsId, agent.id);
  const [choice, setChoice] = useState("");
  const items = models.data?.items ?? [];
  return (
    <div className="space-y-2">
      <p>
        The key works, but no model is chosen yet. A model is the specific AI that answers. Pick one of the models your key serves.
      </p>
      {models.isError ? <Notice tone="danger" role="alert">{errorMessage(models.error)}</Notice> : null}
      <Field id="guide-model-select" label="Model">
        <select id="guide-model-select" className={inputClass} value={choice} onChange={(e) => setChoice(e.target.value)} disabled={items.length === 0}>
          <option value="">{items.length === 0 ? "No models available" : "Select a model"}</option>
          {items.map((m) => (
            <option key={`${m.endpoint_id}/${m.model_id}`} value={`${m.endpoint_id}\u0000${m.model_id}`}>
              {m.display_name ?? m.model_id} ({m.provider})
            </option>
          ))}
        </select>
      </Field>
      {patch.error ? <Notice tone="danger" role="alert">{errorMessage(patch.error)}</Notice> : null}
      <Button
        size="sm"
        disabled={!choice || patch.isPending}
        onClick={() => {
          const [model_endpoint_id, model_id] = choice.split("\u0000");
          patch.mutate(
            { model_endpoint_id, model_id },
            { onSuccess: () => void qc.invalidateQueries({ queryKey: grantKeys.readiness(agent.id) }) },
          );
        }}
      >
        {patch.isPending ? "Saving…" : "Use this model"}
      </Button>
    </div>
  );
}

/** Optional next steps below the guide on the Welcome page. */
export function OptionalNext({ org, ws, handle, isOwner }: { org: string; ws: string; handle?: string; isOwner: boolean }) {
  const base = `/o/${org}/w/${ws}`;
  const items = [
    { href: `${base}/apps`, label: "Connect an app", text: "Let the agent use GitHub, Linear and more, with your approval." },
    handle ? { href: `${base}/agents/${handle}?tab=instructions`, label: "Write instructions for the agent", text: "Tell it what you want it to do and how." } : null,
    handle ? { href: `${base}/agents/${handle}?tab=schedules`, label: "Set up a schedule", text: "Have it do something every morning." } : null,
    isOwner ? { href: `/o/${org}/members`, label: "Invite a teammate", text: "Share this workspace with people you work with." } : null,
  ].filter((x): x is { href: string; label: string; text: string } => x !== null);
  return (
    <Section id="optional" title="Next, when you are ready" description="None of this is needed to start.">
      <ul className="grid gap-3 sm:grid-cols-2">
        {items.map((i) => (
          <li key={i.href}>
            <Link href={i.href} className="block h-full rounded-card border border-line bg-surface p-4 shadow-card hover:border-line-strong">
              <span className="block text-sm font-semibold text-ink">{i.label}</span>
              <span className="mt-1 block text-sm text-ink-2">{i.text}</span>
            </Link>
          </li>
        ))}
      </ul>
    </Section>
  );
}
