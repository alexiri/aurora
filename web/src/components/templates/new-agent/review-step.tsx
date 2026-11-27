"use client";

import type { FileView, SkillRefPlan, TemplatePlan } from "@/lib/api/agent-templates";
import { cn } from "@/lib/utils";
// Not a hook, whatever its name says.
import { descriptionSentence, useItToLine as itToLine } from "@/lib/skill-name";

import { Fold } from "@/components/ui/fold";
import { Notice } from "@/components/ui/notice";
import { SwitchField } from "@/components/ui/switch";

import { notAddedWords, sha7 } from "./words";

// P6-TPL-4 Part B1, C-1116 (the Review step): what <Agent> can do (C-986's summary, over the accounts as chosen),
// then the template's parts in full, as data, folded per part, and **Skills from libraries** with their content at
// the commit. Everything third-party is shown as inert text: it is read, never rendered and never run (SEC-146).

const Text = ({ children, className }: { children: string; className?: string }) => (
  <p className={cn("whitespace-pre-wrap text-ink-2 [overflow-wrap:anywhere]", className)}>{children}</p>
);

const FileLine = ({ f }: { f: FileView }) => (
  <p className="text-xs text-muted">
    <span className="font-mono">{f.path}</span>
    {f.executable ? " (a script)" : ""} · {f.size} bytes
  </p>
);

function Files({ files }: { files: FileView[] }) {
  return (
    <>
      {files.map((f) => (
        <div key={f.path} className="space-y-1">
          <FileLine f={f} />
          {f.text ? <Text className="rounded-control border border-line bg-subtle p-2 font-mono text-xs">{f.text}</Text> : null}
        </div>
      ))}
    </>
  );
}

/** The Review step (C-1116). The summary is computed by the page (it owns the accounts and the Internet access). */
export function ReviewStep({
  plan,
  agentName,
  summary,
  adds,
  onAddRef,
  errors = {},
}: {
  plan: TemplatePlan;
  agentName: string;
  /** The C-986 sentences over the planned accounts and the workspace's Internet access. */
  summary: string[];
  /** Per reference index, whether the person adds it (absent = the default, add an available one). */
  adds: Record<number, boolean>;
  onAddRef: (index: number, add: boolean) => void;
  errors?: Record<string, string>;
}) {
  const content = plan.content;
  const libraries = plan.skill_refs.filter((r) => r.kind === "library");
  return (
    <div className="space-y-4" data-testid="review-step">
      <section className="space-y-1" aria-label={`What ${agentName} can do`}>
        <h3 className="font-display text-base font-semibold text-ink">What {agentName} can do</h3>
        {summary.length === 0 ? <p className="text-sm text-ink-2">Nothing yet — finish the steps above.</p> : null}
        {summary.map((s) => (
          <p key={s} className="text-sm text-ink-2">
            {s}
          </p>
        ))}
      </section>

      <Fold summary="Instructions" data-testid="review-instructions">
        <Text>{content.instructions}</Text>
      </Fold>

      {content.skills.length > 0 ? (
        <Fold summary={`Skills (${content.skills.length})`} data-testid="review-skills">
          <div className="space-y-4 pt-2">
            {content.skills.map((s) => (
              <div key={s.name} className="space-y-2 border-t border-line pt-2 first:border-t-0 first:pt-0">
                <p className="text-sm font-semibold text-ink">{s.name}</p>
                {s.use_it_when ? <Text>{itToLine(s.use_it_when)}</Text> : null}
                <Text>{s.body}</Text>
                <Files files={s.files} />
              </div>
            ))}
          </div>
        </Fold>
      ) : null}

      {content.helpers.length > 0 ? (
        <Fold summary={`Helper files (${content.helpers.length})`} data-testid="review-helpers">
          <div className="space-y-3 pt-2">
            {content.helpers.map((f) => (
              <div key={f.path} className="space-y-1">
                <FileLine f={f} />
                {f.text ? <Text className="rounded-control border border-line bg-subtle p-2 font-mono text-xs">{f.text}</Text> : null}
              </div>
            ))}
          </div>
        </Fold>
      ) : null}

      {content.memories.length > 0 ? (
        <Fold summary={`Memories (${content.memories.length})`} data-testid="review-memories">
          <div className="space-y-3 pt-2">
            {content.memories.map((m) => (
              <div key={m.key} className="space-y-1">
                <p className="text-sm font-semibold text-ink">
                  <span className="font-mono text-xs">{m.key}</span>
                  {m.locked ? <span className="font-normal text-muted"> locked</span> : ""}
                </p>
                <Text>{m.content}</Text>
              </div>
            ))}
          </div>
        </Fold>
      ) : null}

      {libraries.length > 0 ? (
        <section className="space-y-2" aria-label="Skills from libraries">
          <h3 className="font-display text-base font-semibold text-ink">Skills from libraries</h3>
          {libraries.map((ref) => (
            <LibraryRef key={ref.index} skillRef={ref} added={adds[ref.index] ?? true} onAdd={(add) => onAddRef(ref.index, add)} />
          ))}
          {errors.skill_refs ? (
            <p className="text-xs text-danger-ink" role="alert">
              {errors.skill_refs}
            </p>
          ) : null}
        </section>
      ) : null}
    </div>
  );
}

/** One library reference (C-1142): its library and the skill's content at the commit, as data; optional ones switch. */
function LibraryRef({ skillRef, added, onAdd }: { skillRef: SkillRefPlan; added: boolean; onAdd: (add: boolean) => void }) {
  const library = skillRef.library;
  if (!skillRef.available) {
    const words = notAddedWords(skillRef);
    return skillRef.required ? (
      <Notice tone="pending">
        <p>{words}</p>
      </Notice>
    ) : (
      <p className="text-sm text-ink-2" data-testid={`ref-${skillRef.index}`}>
        {words}
      </p>
    );
  }
  return (
    <div className="space-y-2 rounded-card border border-line bg-surface p-4 text-sm" data-testid={`ref-${skillRef.index}`}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="font-semibold text-ink">{skillRef.name}</p>
        {!skillRef.required ? <SwitchField id={`ref-${skillRef.index}`} label={`Add ${skillRef.name}`} checked={added} onChange={onAdd} /> : null}
      </div>
      {skillRef.why ? <Text>{skillRef.why}</Text> : null}
      {library ? (
        <p className="text-xs text-muted">
          From {library.label} ({library.repo}) at {sha7(library.commit_sha)}
        </p>
      ) : null}
      {added ? (
        <div className="space-y-2">
          {/* Repository text stays as its author wrote it. */}
          {skillRef.content?.use_it_when ? <Text>{descriptionSentence(skillRef.content.use_it_when)}</Text> : null}
          {skillRef.content?.body ? <Text>{skillRef.content.body}</Text> : null}
          {skillRef.content ? <Files files={skillRef.content.files} /> : null}
        </div>
      ) : null}
    </div>
  );
}
