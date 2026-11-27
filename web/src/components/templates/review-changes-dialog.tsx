"use client";

import Link from "next/link";
import { useState } from "react";

import { levelLabel } from "@/lib/accounts";
import {
  useApplyAgentTemplate,
  type InputNeededView,
  type OfferPart,
  type SkillRefPlan,
  type TemplateUpdateOffer,
  type UpdateAppEntry,
} from "@/lib/api/agent-templates";
import { ApiError } from "@/lib/api/client";

import { notAddedWords, sha7, type TemplateInput } from "./new-agent/words";
import { TemplateReadme } from "./template-readme";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Fold } from "@/components/ui/fold";
import { DiffView } from "@/components/ui/diff-view";
import { ErrorNotice, Notice } from "@/components/ui/notice";

/**
 * (C-1117) **Review changes**, the dialog behind the agent's Template section: the new version's
 * README in a fold, then every part that differs with its state in words and the full diff, the
 * checkboxes (ticked for `unchanged_here` and `new`), the inputs needed, the apps added with
 * **Give access** links, **Apply** and **Skip this version**. Everything the template brings is
 * inert text (SEC-146); apply sends exactly the ticked parts, bound to the shown hash (C-1112).
 */
export function ReviewChangesDialog({
  agentId,
  agentName,
  templateName,
  offer,
  agentBase,
  onStale,
  onClose,
}: {
  agentId: string;
  agentName: string;
  templateName: string;
  offer: TemplateUpdateOffer;
  /** The agent's own page: the **Give access** links open the Access tab's journey there. */
  agentBase: string;
  /** A 409 `template_version_changed`: the offer is refetched and the section says so (C-1112). */
  onStale: () => void;
  onClose: () => void;
}) {
  const apply = useApplyAgentTemplate(agentId);
  // Per part, whether the person takes it: ticked for `unchanged_here` and `new`, unticked otherwise (C-1117).
  const [taken, setTaken] = useState<Record<string, boolean>>(() => Object.fromEntries(offer.parts.map((p) => [partKey(p), defaultTaken(p.state)])));
  const [inputs, setInputs] = useState<Record<string, string>>(() => Object.fromEntries(offer.inputs_needed.map((i) => [i.id, i.default ?? ""])));
  const requiredEmpty = offer.inputs_needed.some((i) => i.required && !(inputs[i.id] ?? "").trim());

  function body() {
    const parts = offer.parts.filter((p) => isTaken(p, taken));
    return {
      version: offer.version,
      content_sha256: offer.content_sha256,
      parts: parts.map((p) => ({ kind: p.kind, key: p.key })),
      // A new library skill that is taken is added through the library add path (C-1112): its
      // reference carries the library the offer resolved and the commit the person reviewed.
      skill_refs: parts
        .filter((p) => p.kind === "library_skill" && p.state === "new" && p.skill_ref?.library)
        .map((p) => ({ index: p.skill_ref!.index, library_id: p.skill_ref!.library!.id, commit_sha: p.skill_ref!.library!.commit_sha })),
      inputs: Object.fromEntries(Object.entries(inputs).filter(([, v]) => v.trim() !== "")),
    };
  }

  return (
    <Dialog open onOpenChange={(o) => !o && !apply.isPending && onClose()}>
      <DialogContent
        title="Review changes"
        description={`Version ${offer.version} of ${templateName}. ${agentName} keeps working as it is until you apply it.`}
        size="lg"
        fixedHeight="600px"
        footer={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <Button type="button" variant="outline" disabled={apply.isPending} onClick={() => apply.mutate({ version: offer.version, content_sha256: offer.content_sha256, parts: [] })}>
              Skip this version
            </Button>
            <Button
              type="button"
              disabled={apply.isPending || requiredEmpty}
              onClick={() =>
                apply.mutate(body(), {
                  onSuccess: () => onClose(),
                  onError: (e) => {
                    if (e instanceof ApiError && e.code === "template_version_changed") onStale();
                  },
                })
              }
            >
              {apply.isPending ? "Applying…" : "Apply"}
            </Button>
          </div>
        }
      >
        <div className="space-y-4 text-sm text-ink-2" data-testid="review-changes">
          {apply.isError ? <ErrorNotice error={apply.error} /> : null}
          <Fold summary="About this template" data-testid="offer-readme">
            <p className="pt-2 text-xs text-muted">
              Text from {templateName} {offer.version}. It is shown, never applied.
            </p>
            <div className="pt-2">
              <TemplateReadme source={offer.readme} />
            </div>
          </Fold>
          {offer.parts.length === 0 ? <p>Nothing else changes: the two versions differ only in what is shown above.</p> : null}
          {offer.parts.map((p, i) => (
            <OfferPartView key={partKey(p)} index={i} part={p} agentName={agentName} taken={isTaken(p, taken)} onTaken={(v) => setTaken((t) => ({ ...t, [partKey(p)]: v }))} />
          ))}
          {offer.inputs_needed.length > 0 ? (
            <section className="space-y-2" aria-label="Inputs" data-testid="offer-inputs">
              <h3 className="font-display text-base font-semibold text-ink">Inputs</h3>
              <p>The new version needs these to fill in its texts.</p>
              {offer.inputs_needed.map((i) => (
                <InputField key={i.id} input={i} value={inputs[i.id] ?? ""} onChange={(v) => setInputs((s) => ({ ...s, [i.id]: v }))} />
              ))}
            </section>
          ) : null}
          {offer.apps_added.length > 0 ? (
            <section className="space-y-2" aria-label="Apps to give access" data-testid="offer-apps">
              <h3 className="font-display text-base font-semibold text-ink">New apps</h3>
              <p>The update does not give access. {agentName} can use these once you give access on the Apps &amp; access page.</p>
              {offer.apps_added.map((a, i) => (
                <AppEntry key={i} entry={a} agentBase={agentBase} />
              ))}
            </section>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** A checkbox key that cannot collide: the kind and the key are a pair, so they are packed as JSON. */
function partKey(p: OfferPart): string {
  return JSON.stringify([p.kind, p.key]);
}

function defaultTaken(state: string): boolean {
  return state === "unchanged_here" || state === "new";
}

/** A new library skill the workspace cannot reach has no checkbox (C-1117), so it is never taken. */
function isTaken(p: OfferPart, taken: Record<string, boolean>): boolean {
  if (p.kind === "library_skill" && p.state === "new" && p.skill_ref && !p.skill_ref.available) return false;
  return taken[partKey(p)] ?? defaultTaken(p.state);
}

/** The words of a part's state (C-1112): what the state says about the agent's copy and the new version. */
function stateWords(state: string, agentName: string): string {
  switch (state) {
    case "unchanged_here":
      return "The template changed it; you have not. Tick to take the new text.";
    case "edited_here":
      return "You changed it since. Leave it unticked to keep yours.";
    case "removed_here":
      return `It is not on ${agentName} any more. Tick to add it back.`;
    case "new":
      return "New in the template.";
    case "removed_upstream":
      return `The new version does not have it any more. Tick to remove it from ${agentName}.`;
    default:
      return state;
  }
}

function headingOf(p: OfferPart): string {
  switch (p.kind) {
    case "instructions":
      return "Instructions";
    case "helpers":
      return "Helper files";
    case "skill":
      return `Skill ${p.key}`;
    case "library_skill":
      return p.skill_ref?.name ?? p.key;
    default:
      return p.key || p.kind;
  }
}

/** One part of the offer: its heading, its state in words, the full diff, and the checkbox (C-1117). */
function OfferPartView({
  index,
  part,
  agentName,
  taken,
  onTaken,
}: {
  index: number;
  part: OfferPart;
  agentName: string;
  taken: boolean;
  onTaken: (v: boolean) => void;
}) {
  const heading = headingOf(part);
  const ref = part.skill_ref;
  // A new library skill that the workspace cannot reach: no checkbox, the sentence instead (C-1117).
  const unavailable = part.kind === "library_skill" && part.state === "new" && ref != null && !ref.available;
  const text = part.kind === "library_skill" ? null : part;
  return (
    <section className="space-y-2 rounded-card border border-line bg-surface p-4" data-testid={`offer-part-${index}`} aria-label={heading}>
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <h3 className="font-display text-base font-semibold text-ink">{heading}</h3>
        {unavailable ? null : <CheckboxField id={`take-part-${index}`} label="Take this change" checked={taken} onChange={onTaken} />}
      </div>
      <p data-testid="part-state">{stateWords(part.state, agentName)}</p>
      {ref ? <RefLine skillRef={ref} unavailable={unavailable} /> : null}
      {text?.current != null && text.proposed != null && text.current !== text.proposed ? (
        <DiffView old={text.current} next={text.proposed} label={`Diff of ${heading}, current against the new version`} fileName={heading} />
      ) : null}
      {text?.current == null && text?.proposed != null ? (
        <pre tabIndex={0} aria-label={`${heading} in the new version`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {text.proposed}
        </pre>
      ) : null}
      {text?.proposed == null && text?.current != null ? (
        <pre tabIndex={0} aria-label={`${heading} as ${agentName} has it now`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {text.current}
        </pre>
      ) : null}
    </section>
  );
}

/** A library skill's reference (C-1142): where it comes from and its why; content is never part of an update. */
function RefLine({ skillRef, unavailable }: { skillRef: SkillRefPlan; unavailable: boolean }) {
  const library = skillRef.library;
  if (unavailable) {
    return (
      <Notice tone="pending">
        <p data-testid="ref-unavailable">
          {notAddedWords({ name: skillRef.name, required: true, repo: skillRef.repo, unavailable_reason: skillRef.unavailable_reason })}
        </p>
      </Notice>
    );
  }
  return (
    <div className="space-y-1">
      {skillRef.why ? <p>{skillRef.why}</p> : null}
      {library ? (
        <p className="text-xs text-muted">
          From {library.label} ({library.repo}) at {sha7(library.commit_sha)}. Its content is the library&apos;s and is never part of this update; the
          library proposes its own changes.
        </p>
      ) : null}
    </div>
  );
}

/** One input the new version needs (C-1112): text or choice, with the default prefilled. */
function InputField({ input, value, onChange }: { input: InputNeededView; value: string; onChange: (v: string) => void }) {
  const t = input as TemplateInput;
  const id = `offer-input-${input.id}`;
  return (
    <Field id={id} label={input.required ? input.label : `${input.label} (optional)`} hint={input.help ?? undefined}>
      {t.kind === "choice" ? (
        <select id={id} className={inputClass} value={value} onChange={(e) => onChange(e.target.value)}>
          {input.required && !t.default ? <option value="">Choose…</option> : null}
          {(t.options ?? []).map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
      ) : (
        <input id={id} className={inputClass} value={value} maxLength={input.max_length ?? 100} onChange={(e) => onChange(e.target.value)} />
      )}
    </Field>
  );
}

/** One app the offered version needs: its access in words and the **Give access** link to the Access tab's journey. */
function AppEntry({ entry, agentBase }: { entry: UpdateAppEntry; agentBase: string }) {
  const level = entry.level === "read" || entry.level === "ask" || entry.level === "act" ? levelLabel(entry.level) : null;
  const appId = entry.app_ids[0] ?? "";
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 rounded-control border border-line p-3" data-testid="offer-app">
      <div className="min-w-0 space-y-0.5">
        <p className="font-medium text-ink">{entry.app_ids.join(", ")}</p>
        <p className="text-xs text-muted">
          {level ? `Access: ${level}. ` : ""}
          {entry.why}
        </p>
      </div>
      <Link href={`${agentBase}?tab=access&connect=${encodeURIComponent(appId)}`} className="text-sm font-medium text-link underline">
        Give access
      </Link>
    </div>
  );
}
