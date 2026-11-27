"use client";

import Link from "next/link";
import { useState } from "react";

import { levelLabel } from "@/lib/accounts";
import {
  useApplyAgentTemplate,
  type ApplyResolution,
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

/** The size cap the apply server enforces on a free edit of one conflict hunk (FIX-251). */
const RESOLUTION_TEXT_MAX = 20_000;

/**
 * (C-1117) **Review changes**, the dialog behind the agent's Template section: the new version's
 * README in a fold, then every part that differs with its state in words and the full diff, the
 * checkboxes (ticked for `unchanged_here`, `new` and a clean merge), the inputs needed, the apps
 * added with **Give access** links, **Apply** and **Skip this version**. Since FIX-251 a part the
 * person edited is offered as a three-way merge: a clean merge is ticked and shown diffed against
 * the current text; a conflict is resolved hunk by hunk — **Keep mine**, **Take the new one** or a
 * free edit — before its checkbox may apply anything. Memories are never part of an update.
 * Everything the template brings is inert text (SEC-146); apply sends exactly the ticked parts,
 * bound to the shown hash (C-1112).
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
  // Per part, whether the person takes it: ticked for `unchanged_here`, `new` and `merged`,
  // unticked otherwise (C-1117, FIX-251) — nothing of a conflict applies until it is resolved.
  const [taken, setTaken] = useState<Record<string, boolean>>(() => Object.fromEntries(offer.parts.map((p) => [partKey(p), defaultTaken(p.state)])));
  // (FIX-251) The person's choice for every conflict hunk: keyed by part, then the hunk's index in
  // the offer's `conflicts` — one resolution per hunk, in the offer's order, sent with the apply.
  const [resolutions, setResolutions] = useState<Record<string, Record<number, ApplyResolution>>>({});
  const [inputs, setInputs] = useState<Record<string, string>>(() => Object.fromEntries(offer.inputs_needed.map((i) => [i.id, i.default ?? ""])));
  const requiredEmpty = offer.inputs_needed.some((i) => i.required && !(inputs[i.id] ?? "").trim());
  // A ticked `conflict` part whose hunks are not all chosen cannot apply (the server would refuse
  // it with `template_merge_unresolved`): Apply waits until every change has a choice.
  const unresolved = offer.parts.some(
    (p) =>
      isTaken(p, taken) &&
      p.merge?.state === "conflict" &&
      (p.merge.conflicts ?? []).some((_, i) => resolutions[partKey(p)]?.[i] == null),
  );

  function body() {
    const parts = offer.parts.filter((p) => isTaken(p, taken));
    return {
      version: offer.version,
      content_sha256: offer.content_sha256,
      parts: parts.map((p) => ({
        kind: p.kind,
        key: p.key,
        // (FIX-251) A part whose offer carries a merge echoes the merge's hash the review showed;
        // a `conflict` part adds one resolution per conflict hunk, in the offer's order.
        ...(p.merge ? { merge_sha256: p.merge.merge_sha256 } : {}),
        ...(p.merge && p.merge.state === "conflict"
          ? {
              resolutions: (p.merge.conflicts ?? []).map((c, i) => {
                const r = resolutions[partKey(p)]?.[i];
                return r ?? { unit: c.unit, choice: "keep_mine" };
              }),
            }
          : {}),
      })),
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
              disabled={apply.isPending || requiredEmpty || unresolved}
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
          <p data-testid="memories-note">Memories are never changed by an update.</p>
          {unresolved ? (
            <Notice tone="pending" role="status" data-testid="merge-unresolved">
              <p>Every change still waiting needs a choice: Keep mine, Take the new one, or your own text. Or leave the part unticked to keep it as it is.</p>
            </Notice>
          ) : null}
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
            <OfferPartView
              key={partKey(p)}
              index={i}
              part={p}
              agentName={agentName}
              taken={isTaken(p, taken)}
              onTaken={(v) => setTaken((t) => ({ ...t, [partKey(p)]: v }))}
              resolutions={resolutions[partKey(p)]}
              onResolve={(hunk, r) => setResolutions((s) => ({ ...s, [partKey(p)]: { ...(s[partKey(p)] ?? {}), [hunk]: r } }))}
            />
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
  return state === "unchanged_here" || state === "new" || state === "merged";
}

/** A new library skill the workspace cannot reach has no checkbox (C-1117), so it is never taken. */
function isTaken(p: OfferPart, taken: Record<string, boolean>): boolean {
  if (p.kind === "library_skill" && p.state === "new" && p.skill_ref && !p.skill_ref.available) return false;
  return taken[partKey(p)] ?? defaultTaken(p.state);
}

/** The words of a part's state (C-1112, FIX-251): what the state says about the agent's copy and the new version. */
function stateWords(state: string, agentName: string): string {
  switch (state) {
    case "unchanged_here":
      return "The template changed it; you have not. Tick to take the new text.";
    case "merged":
      return "Merged with your changes: your edit stays, and the new version's change is applied around it. Tick to apply the merged text; leave it unticked to keep yours.";
    case "conflict":
      return "You both changed it. Choose Keep mine or Take the new one for every change below — or write your own text — then tick to apply; leave it unticked to keep yours.";
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

/** A conflict hunk's unit in words: the part's own text, a skill's description or body, or a file's path. */
function unitLabel(unit: string, heading: string): string {
  if (unit === "text") return heading;
  if (unit === "description") return "Description";
  if (unit === "body") return "Body";
  if (unit.startsWith("file:")) return unit.slice("file:".length);
  return unit;
}

/** One part of the offer: its heading, its state in words, the diff (a merge's result, or the
 * full texts), a conflict's hunks with their choices, and the checkbox (C-1117, FIX-251). */
function OfferPartView({
  index,
  part,
  agentName,
  taken,
  onTaken,
  resolutions,
  onResolve,
}: {
  index: number;
  part: OfferPart;
  agentName: string;
  taken: boolean;
  onTaken: (v: boolean) => void;
  /** The person's choice per conflict hunk, by hunk index; absent where nothing is chosen yet. */
  resolutions: Record<number, ApplyResolution> | undefined;
  onResolve: (hunk: number, r: ApplyResolution) => void;
}) {
  const heading = headingOf(part);
  const ref = part.skill_ref;
  // A new library skill that the workspace cannot reach: no checkbox, the sentence instead (C-1117).
  const unavailable = part.kind === "library_skill" && part.state === "new" && ref != null && !ref.available;
  const text = part.kind === "library_skill" ? null : part;
  const merge = part.merge;
  return (
    <section className="space-y-2 rounded-card border border-line bg-surface p-4" data-testid={`offer-part-${index}`} aria-label={heading}>
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <h3 className="font-display text-base font-semibold text-ink">{heading}</h3>
        {unavailable ? null : <CheckboxField id={`take-part-${index}`} label="Take this change" checked={taken} onChange={onTaken} />}
      </div>
      <p data-testid="part-state">{stateWords(part.state, agentName)}</p>
      {/* (FIX-251) Why a part the person edited has no merge, in the server's words. */}
      {part.merge_note ? <p data-testid="merge-note">{part.merge_note}</p> : null}
      {ref ? <RefLine skillRef={ref} unavailable={unavailable} /> : null}
      {/* (FIX-251) A clean merge: the merged text diffed against what the agent has now. */}
      {merge?.state === "merged" && merge.result != null && text?.current != null && text.current !== merge.result ? (
        <DiffView old={text.current} next={merge.result} label={`Diff of ${heading}, current against the merged text`} fileName={heading} />
      ) : null}
      {/* A clean merge that changes nothing, or a part the merge cannot improve: the texts as they are. */}
      {merge?.state === "merged" && text?.current != null && text.current === merge.result ? (
        <pre tabIndex={0} aria-label={`${heading}, merged`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {merge.result}
        </pre>
      ) : null}
      {/* (FIX-251) A conflict: every hunk with its own choice. */}
      {merge?.state === "conflict" ? (
        <div className="space-y-3" data-testid="merge-conflicts">
          {(merge.conflicts ?? []).map((c, i) => (
            <ConflictHunk key={i} group={partKey(part)} index={i} hunk={c} heading={heading} value={resolutions?.[i]} onResolve={onResolve} />
          ))}
        </div>
      ) : null}
      {/* No merge (the fallback, or nothing to diff): the full texts as before (C-1117). */}
      {!merge && text?.current != null && text.proposed != null && text.current !== text.proposed ? (
        <DiffView old={text.current} next={text.proposed} label={`Diff of ${heading}, current against the new version`} fileName={heading} />
      ) : null}
      {!merge && text?.current == null && text?.proposed != null ? (
        <pre tabIndex={0} aria-label={`${heading} in the new version`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {text.proposed}
        </pre>
      ) : null}
      {!merge && text?.proposed == null && text?.current != null ? (
        <pre tabIndex={0} aria-label={`${heading} as ${agentName} has it now`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {text.current}
        </pre>
      ) : null}
    </section>
  );
}

/** One conflict hunk (FIX-251): both sides diffed, then the choice — **Keep mine**, **Take the new
 * one**, or a free edit of the person's own text, shown diffed against what the agent has now. */
function ConflictHunk({
  group,
  index,
  hunk,
  heading,
  value,
  onResolve,
}: {
  group: string;
  index: number;
  hunk: { unit: string; ours: string; theirs: string };
  heading: string;
  value: ApplyResolution | undefined;
  onResolve: (hunk: number, r: ApplyResolution) => void;
}) {
  const unit = unitLabel(hunk.unit, heading);
  const set = (choice: string, text?: string) => onResolve(index, text === undefined ? { unit: hunk.unit, choice } : { unit: hunk.unit, choice, text });
  const editing = value?.choice === "text";
  const editText = editing ? (value.text ?? hunk.ours) : "";
  return (
    <div className="space-y-2 rounded-control border border-line p-3" data-testid={`merge-conflict-${index}`}>
      <p className="font-medium text-ink">{unit}</p>
      <DiffView old={hunk.ours} next={hunk.theirs} label={`The change in ${unit}: what ${heading} has now against the new version`} fileName={unit} />
      <div role="radiogroup" aria-label={`Resolve the change in ${unit}`} className="flex flex-wrap gap-x-6 gap-y-1">
        <Radio id={`resolve-${index}-mine`} name={group} label="Keep mine" checked={value?.choice === "keep_mine"} onChange={() => set("keep_mine")} />
        <Radio id={`resolve-${index}-theirs`} name={group} label="Take the new one" checked={value?.choice === "take_theirs"} onChange={() => set("take_theirs")} />
        <Radio id={`resolve-${index}-text`} name={group} label="Edit your own text" checked={editing} onChange={() => set("text", value?.text ?? hunk.ours)} />
      </div>
      {editing ? (
        <div className="space-y-2">
          <label htmlFor={`resolve-${index}-edit`} className="block text-sm font-medium text-ink">
            Your text
          </label>
          <textarea
            id={`resolve-${index}-edit`}
            rows={4}
            maxLength={RESOLUTION_TEXT_MAX}
            spellCheck={false}
            className={`${inputClass} font-mono`}
            value={editText}
            onChange={(e) => set("text", e.target.value)}
          />
          {editText !== hunk.ours ? (
            <DiffView old={hunk.ours} next={editText} label={`Diff of your edit of ${unit}, what ${heading} has now against what you wrote`} fileName={unit} />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** One radio with its label as one tap target (the conflict choice's own group, per hunk). */
function Radio({ id, name, label, checked, onChange }: { id: string; name: string; label: string; checked: boolean; onChange: () => void }) {
  return (
    <label htmlFor={id} className="flex min-h-9 items-center gap-2 text-sm text-ink-2">
      <input id={id} type="radio" name={name} checked={checked} onChange={onChange} className="size-4 shrink-0 accent-accent" />
      {label}
    </label>
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
