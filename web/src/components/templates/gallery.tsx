"use client";

import { useState } from "react";

import { Avatar } from "@/components/agent-avatar";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { CardGrid } from "@/components/ui/card-grid";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { AppIcon } from "@/components/ui/app-icon";
import { Notice } from "@/components/ui/notice";
import { useTemplatePlan, useWorkspaceTemplates, templateAvatarUrl, type TemplateCard } from "@/lib/api/agent-templates";
import { cn } from "@/lib/utils";

import { TemplateReadme } from "./template-readme";
import { notReadyWords, useAppName } from "./words";

// P6-TPL-4 Part B1, C-1116 (D-458): **More templates…** opens the gallery of the workspace's templates; choosing a
// card opens its details first, so people can decide before deploying; **Use this template** opens the four-step page.

/**
 * What a card says a template runs on its own (C-1116): every automation, by name. A webhook arrives paused, so the
 * line says so.
 */
export function runsOnItsOwn(automations: { kind: string; name: string }[]): string[] {
  return automations.map((a) => (a.kind === "webhook" ? `Webhook ${a.name}, created paused` : `Schedule ${a.name}`));
}

function AutomationLines({ automations }: { automations: { kind: string; name: string }[] }) {
  const lines = runsOnItsOwn(automations);
  if (lines.length === 0) return null;
  return (
    <div className="text-sm text-ink-2">
      <p className="font-medium text-ink">What it runs on its own</p>
      <ul className="mt-1 list-inside list-disc space-y-0.5">
        {lines.map((l) => (
          <li key={l}>{l}</li>
        ))}
      </ul>
    </div>
  );
}

function AppIcons({ card, appName, size = 24 }: { card: TemplateCard; appName: (id: string) => string; size?: number }) {
  const ids = [...new Set(card.apps.flatMap((a) => a.app_ids))];
  if (ids.length === 0) return null;
  return (
    <span className="flex flex-wrap items-center gap-1" aria-label="Apps it needs">
      {ids.map((id) => (
        <AppIcon key={id} appId={id} name={appName(id)} size={size} />
      ))}
    </span>
  );
}

/** One card of the gallery (C-1116): avatar, name, summary, app icons, what it runs on its own, and the not-ready sentences. */
export function TemplateCardView({ card, wsId, appName, onOpen }: { card: TemplateCard; wsId: string; appName: (id: string) => string; onOpen: () => void }) {
  const missing = notReadyWords(card, appName);
  return (
    <li className="flex h-full flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <button
        type="button"
        onClick={onOpen}
        className="-m-1 flex min-w-0 flex-1 flex-col items-start gap-3 rounded-control p-1 text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
      >
        <span className="flex items-center gap-3">
          <Avatar name={card.name} url={card.has_avatar ? templateAvatarUrl(wsId, card.template_id) : null} tintKey={card.template_id} size={48} />
          <span className="min-w-0">
            <span className="block font-display text-base font-semibold text-ink">{card.name}</span>
            <span className="block text-sm text-muted">Version {card.version}</span>
          </span>
        </span>
        <span className="text-sm text-ink-2 [overflow-wrap:anywhere]">{card.summary}</span>
        <AppIcons card={card} appName={appName} />
        <AutomationLines automations={card.automations} />
        {missing.length > 0 ? (
          <div className="text-sm text-pending-ink">
            {missing.map((m) => (
              <p key={m}>{m}</p>
            ))}
          </div>
        ) : null}
      </button>
    </li>
  );
}

/**
 * The template's details view (D-458, C-1116), before anything else: the name, avatar and summary; **About this
 * template** (the README, rendered the C-1116 way); what it needs; what it runs on its own; its helper files by
 * name; and the version. Then **Use this template** or **Back**.
 */
export function TemplateDetails({ card, wsId, onBack, onUse }: { card: TemplateCard; wsId: string; onBack: () => void; onUse: () => void }) {
  const appName = useAppName();
  const plan = useTemplatePlan(wsId, card.template_id);
  const missing = notReadyWords(card, appName);
  const helpers = plan.data?.content?.helpers ?? [];
  return (
    <div className="space-y-4" data-testid="template-details">
      <div className="flex items-center gap-3">
        <Avatar name={card.name} url={card.has_avatar ? templateAvatarUrl(wsId, card.template_id) : null} tintKey={card.template_id} size={48} />
        <div className="min-w-0">
          <h3 className="font-display text-base font-semibold text-ink">{card.name}</h3>
          <p className="text-sm text-muted">Version {card.version}</p>
        </div>
      </div>
      <p className="text-sm text-ink-2 [overflow-wrap:anywhere]">{card.summary}</p>

      <section className="space-y-1" aria-label="About this template">
        <h4 className="text-sm font-semibold text-ink">About this template</h4>
        <TemplateReadme source={card.readme} />
      </section>

      <section className="space-y-1 text-sm" aria-label="What it needs">
        <h4 className="font-semibold text-ink">What it needs</h4>
        {card.apps.length === 0 && card.skill_refs.length === 0 ? <p className="text-ink-2">Nothing but Aurora.</p> : null}
        {card.apps.map((entry) =>
          entry.app_ids.map((id) => (
            <p key={`${entry.required}-${id}`} className={cn(entry.required && !entry.available ? "text-pending-ink" : "text-ink-2")}>
              {appName(id)}
              {entry.required && !entry.available ? " — isn't available here" : entry.required ? " — required" : " — optional"}
            </p>
          )),
        )}
        {card.skill_refs.map((ref) =>
          ref.kind === "organization" ? (
            <p key={ref.index} className={cn(ref.required && !ref.available ? "text-pending-ink" : "text-ink-2")}>
              Uses your organization&apos;s {ref.name} skill{ref.required ? "" : " (optional)"}
            </p>
          ) : (
            <p key={ref.index} className={cn(ref.required && !ref.available ? "text-pending-ink" : "text-ink-2")}>
              Skill {ref.name}, from a skill library{ref.required ? "" : " (optional)"}
              {ref.required && !ref.available ? " — isn't available here" : ""}
            </p>
          ),
        )}
      </section>

      <AutomationLines automations={card.automations} />

      <section className="text-sm" aria-label="Helper files">
        <h4 className="font-semibold text-ink">Helper files</h4>
        {plan.isPending ? (
          <p className="text-ink-2">Checking…</p>
        ) : helpers.length === 0 ? (
          <p className="text-ink-2">None. Everything the agent needs is in its instructions and skills.</p>
        ) : (
          <ul className="mt-1 list-inside list-disc">
            {helpers.map((f) => (
              <li key={f.path} className="text-ink-2">
                <span className="font-mono text-xs">{f.path}</span>
                {f.executable ? " (a script)" : ""}
              </li>
            ))}
          </ul>
        )}
      </section>

      {missing.length > 0 ? (
        <Notice tone="pending">
          {missing.map((m) => (
            <p key={m}>{m}</p>
          ))}
        </Notice>
      ) : null}

      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="outline" onClick={onBack}>
          Back
        </Button>
        <Button onClick={onUse} disabled={!card.ready || plan.isPending}>
          Use this template
        </Button>
      </div>
      {plan.isError ? <ErrorState error={plan.error} onRetry={() => void plan.refetch()} /> : null}
    </div>
  );
}

/**
 * The gallery (**More templates…**, C-1116): the workspace's available templates as cards. Choosing a card opens its
 * details in the same dialog; **Use this template** calls `onUse`.
 */
export function TemplateGalleryDialog({
  open,
  onOpenChange,
  wsId,
  onUse,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  wsId: string;
  /** Called with the chosen template's id; the host navigates to the four-step page. */
  onUse: (templateId: string) => void;
}) {
  const appName = useAppName();
  const templates = useWorkspaceTemplates(wsId, open);
  const [selected, setSelected] = useState<string | null>(null);
  const card = templates.data?.items.find((t) => t.template_id === selected) ?? null;
  const close = (o: boolean) => {
    onOpenChange(o);
    if (!o) setSelected(null);
  };
  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent
        title={card ? card.name : "More templates"}
        description={card ? undefined : "Ready-made agents your administrator added."}
        size="lg"
        back={
          card ? (
            <button
              type="button"
              aria-label="Back"
              onClick={() => setSelected(null)}
              className="inline-flex size-9 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink"
            >
              <span aria-hidden="true">←</span>
            </button>
          ) : null
        }
      >
        {templates.isPending ? (
          <Loading />
        ) : templates.isError ? (
          <ErrorState error={templates.error} onRetry={() => void templates.refetch()} />
        ) : card ? (
          <TemplateDetails card={card} wsId={wsId} onBack={() => setSelected(null)} onUse={() => onUse(card.template_id)} />
        ) : templates.data.items.length === 0 ? (
          <p className="py-6 text-center text-sm text-ink-2">No templates have been offered here yet.</p>
        ) : (
          <CardGrid aria-label="Agent templates">
            {templates.data.items.map((t) => (
              <TemplateCardView key={t.template_id} card={t} wsId={wsId} appName={appName} onOpen={() => setSelected(t.template_id)} />
            ))}
          </CardGrid>
        )}
      </DialogContent>
    </Dialog>
  );
}
