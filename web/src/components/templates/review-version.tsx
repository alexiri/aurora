"use client";

import { useState, type ReactNode } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DiffView } from "@/components/ui/diff-view";
import { Fold } from "@/components/ui/fold";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { ApiError } from "@/lib/api/client";
import {
  useAdminTemplates,
  useApproveTemplateVersion,
  useRejectTemplateVersion,
  useTemplateVersion,
  type FileView,
  type TemplateAppEntryView,
  type TemplateAutomationView,
  type TemplateContentView,
  type TemplateInputView,
  type TemplateSkillRefView,
  type TemplateVersionDetail,
} from "@/lib/api/agent-templates";
import { diffTemplateContent, type TemplateChange, type TemplateDiff } from "@/lib/template-diff";
import { adminRoutes } from "@/lib/routes";
import { formatCount } from "@/lib/format-date";

import { TemplateReadme } from "./template-readme";
import { useAppName, versionStatusWords } from "./words";

// The review of one version (C-1114, FIX-250): everything of `TemplateContentView` in full, all of
// it marked as text from the source at the commit. Each part's diff sits inside the section it
// belongs to, folded GitLab-style; a part nothing touched is collapsed to one line "Unchanged".
// Every part is data — the review only ever reads.

const LEVEL_WORDS: Record<string, string> = { read: "Read", ask: "Ask", act: "Act" };
const VERIFICATION_WORDS: Record<string, string> = {
  signed: "Verified with a signature.",
  password: "Verified with a password.",
  none: "No verification.",
};

/** The section each change belongs to, for the **Look at these first** links; null when the change
 * has no section of its own (the name, the summary, the effort, …) and stays in the top block. */
function sectionOf(part: string): string | null {
  for (const [prefix, anchor] of [
    ["skill_ref:", "skill-refs"],
    ["skill:", "skills"],
    ["helper:", "helpers"],
    ["memory:", "memories"],
    ["automation:", "automations"],
    ["app:", "apps"],
    ["input:", "inputs"],
  ] as const) {
    if (part.startsWith(prefix)) return anchor;
  }
  return null;
}

/** A text file's meta line (path, size, executable in words); the text itself is the caller's. */
function FileHeading({ f }: { f: FileView }) {
  return (
    <p className="text-sm font-semibold text-ink">
      <span className="font-mono">{f.path}</span>{" "}
      <span className="font-normal text-muted">
        ({formatCount(f.size)} chars{f.executable ? ", executable" : ""})
      </span>
    </p>
  );
}

/** A text file in full; executable is said in words. Nothing is ever run. */
function TextFile({ f, label }: { f: FileView; label: string }) {
  return (
    <div className="space-y-1">
      <FileHeading f={f} />
      {f.text == null ? (
        <p className="text-xs text-muted">Not text, so it is not shown.</p>
      ) : (
        <pre tabIndex={0} aria-label={label} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {f.text}
        </pre>
      )}
    </div>
  );
}

/** The change of one part, in words when there is no text to diff. */
function changeWords(c: TemplateChange): string {
  if (c.detail) return c.detail;
  switch (c.kind) {
    case "added":
      return "New in this version.";
    case "removed":
      return "Not in this version any more.";
    default:
      return "It changed.";
  }
}

const preClass = "max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 text-sm text-ink [overflow-wrap:anywhere]";

/** One text part in place (FIX-250): the folded diff when it changed — with **Show the current
 * text** for the full text of this version — and one line "Unchanged" that expands when it did not. */
function PartText({ change, label, current, ariaLabel }: { change: TemplateChange | null; label: string; current: string; ariaLabel?: string }) {
  const [show, setShow] = useState(false);
  const named = ariaLabel ?? label;
  if (!change) {
    return (
      <Fold summary="Unchanged" data-testid="diff-unchanged">
        <div className="pt-2">
          <pre aria-label={named} className={preClass}>
            {current}
          </pre>
        </div>
      </Fold>
    );
  }
  return (
    <div className="space-y-2" data-testid="part-diff">
      <DiffView old={change.old ?? ""} next={change.new ?? ""} label={`Diff of ${label}`} fileName={label} status={change.kind === "added" ? "added" : change.kind === "removed" ? "removed" : null} />
      <div className="space-y-2">
        <Button type="button" size="sm" variant="outline" aria-expanded={show} onClick={() => setShow(!show)}>
          {show ? "Hide the current text" : "Show the current text"}
        </Button>
        {show ? (
          <pre aria-label={named} className={preClass}>
            {current}
          </pre>
        ) : null}
      </div>
    </div>
  );
}

/** A section's changes that are words only (apps, automations, inputs, skill references): one
 * sentence per change, above the list itself; a change that carries text shows its diff. */
function DetailChanges({ changes }: { changes: TemplateChange[] }) {
  if (changes.length === 0) return null;
  return (
    <ul className="space-y-1 text-sm" data-testid="diff-change">
      {changes.map((c) => (
        <li key={c.part} className="space-y-1">
          <span>
            <span className="font-medium text-ink">{c.label}</span> — {changeWords(c)}
          </span>
          {c.old != null || c.new != null ? (
            <DiffView old={c.old ?? ""} next={c.new ?? ""} label={`Diff of ${c.label}`} status={c.kind === "added" ? "added" : c.kind === "removed" ? "removed" : null} />
          ) : null}
        </li>
      ))}
    </ul>
  );
}

/** What changed since the version people use now (FIX-247), kept short (FIX-250): the one-line
 * summary and **Look at these first**, each entry linking to the section it belongs to. The diffs
 * themselves sit inside their sections. */
function WhatChanged({ diff, baseVersion }: { diff: TemplateDiff; baseVersion: string }) {
  const others = diff.changes.filter((c) => !sectionOf(c.part) && !c.attention);
  return (
    <Section id="what-changed" title={`What changed since ${baseVersion}`}>
      {diff.changes.length === 0 ? (
        <p data-testid="diff-empty">Nothing changed: this version matches {baseVersion}.</p>
      ) : (
        <div className="space-y-3">
          <p className="text-sm text-ink-2" data-testid="diff-summary">
            {diff.summary}.
          </p>
          {diff.attention.length > 0 ? (
            <section aria-label="Look at these first" data-testid="diff-attention" className="space-y-2 rounded-card border border-line bg-surface p-4">
              <h3 className="font-display text-base font-semibold text-ink">Look at these first</h3>
              <p className="text-sm text-ink-2">These widen what the agent can do or reach.</p>
              <ul className="space-y-1 text-sm">
                {diff.attention.map((c) => {
                  const anchor = sectionOf(c.part);
                  return (
                    <li key={c.part}>
                      <span className="font-medium text-ink">
                        {anchor ? (
                          <a href={`#${anchor}`} className="text-link underline">
                            {c.label}
                          </a>
                        ) : (
                          c.label
                        )}
                      </span>{" "}
                      — {changeWords(c)}
                    </li>
                  );
                })}
              </ul>
            </section>
          ) : null}
          {others.length > 0 ? (
            <ul className="space-y-1 text-sm" data-testid="diff-other">
              {others.map((c) => (
                <li key={c.part}>
                  <span className="font-medium text-ink">{c.label}</span> — {changeWords(c)}
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      )}
    </Section>
  );
}

/** One skill and its files: the folded diffs of the parts that changed, every part nothing touched
 * as one line "Unchanged"; a skill nothing touched collapses whole. */
function SkillCard({ s, changes, comparing }: { s: TemplateContentView["skills"][number]; changes: TemplateChange[] | null; comparing: boolean }) {
  const body = changes?.find((c) => c.part === `skill:${s.name}:body`) ?? null;
  const use = changes?.find((c) => c.part === `skill:${s.name}:use_it_when`) ?? null;
  const header = <p className="font-medium text-ink">{s.name}</p>;
  if (comparing && changes != null && changes.length === 0) {
    return (
      <section aria-label={`Skill ${s.name}`} className="space-y-2 rounded-card border border-line bg-surface p-4">
        {header}
        <Fold summary="Unchanged" data-testid="diff-unchanged">
          <div className="space-y-2 pt-2">
            <p className="text-sm text-ink-2">Use it when: {s.use_it_when}</p>
            <pre aria-label={`Skill ${s.name} body`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
              {s.body}
            </pre>
            {s.files.map((f) => (
              <TextFile key={f.path} f={f} label={`File ${f.path} of skill ${s.name}`} />
            ))}
          </div>
        </Fold>
      </section>
    );
  }
  return (
    <section aria-label={`Skill ${s.name}`} className="space-y-2 rounded-card border border-line bg-surface p-4">
      {header}
      <p className="text-sm text-ink-2">Use it when: {s.use_it_when}</p>
      {comparing && changes != null ? (
        <div className="space-y-2">
          {use ? <DiffView old={use.old ?? ""} next={use.new ?? ""} label={`Diff of Skill ${s.name} · Use it when`} /> : null}
          <PartText change={body} label={`Skill ${s.name} body`} current={s.body} />
          {s.files.map((f) =>
            f.text == null ? (
              <TextFile key={f.path} f={f} label={`File ${f.path} of skill ${s.name}`} />
            ) : (
              <div key={f.path} className="space-y-1">
                <FileHeading f={f} />
                <PartText change={changes.find((c) => c.part === `skill:${s.name}:file:${f.path}`) ?? null} label={`File ${f.path} of skill ${s.name}`} current={f.text} />
              </div>
            ),
          )}
        </div>
      ) : (
        <>
          <pre aria-label={`Skill ${s.name} body`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
            {s.body}
          </pre>
          {s.files.map((f) => (
            <TextFile key={f.path} f={f} label={`File ${f.path} of skill ${s.name}`} />
          ))}
        </>
      )}
    </section>
  );
}

/** Every part of one version (C-1114). `diff` is null for a first version (nothing to compare
 * with): the sections then show the text as today. `<Agent>` is the agent the template creates. */
function Content({ d, diff }: { d: TemplateVersionDetail; diff: TemplateDiff | null }) {
  const c = d.content!;
  const agent = c.agent.name;
  const nameOf = useAppName();
  const group = (g: string) => diff?.changes.filter((x) => x.group === g) ?? [];
  const byPart = (part: string) => diff?.changes.find((x) => x.part === part) ?? null;
  /** A section nothing touched collapses to one line "Unchanged" that expands to today's text. */
  const unchanged = (g: string, children: ReactNode) =>
    diff != null && group(g).length === 0 ? (
      <Fold summary="Unchanged" data-testid="diff-unchanged">
        <div className="pt-2">{children}</div>
      </Fold>
    ) : (
      <>{children}</>
    );
  return (
    <>
      <p className="text-sm text-ink-2">
        Text from <span className="font-medium text-ink">{d.source.name}</span> at <span className="font-mono">{d.commit_sha.slice(0, 7)}</span>. The package matches the repository at{" "}
        <span className="font-mono">{d.commit_sha.slice(0, 7)}</span>.
      </p>
      <AboutThisTemplate readme={c.readme} change={byPart("readme")} comparing={diff != null} />
      <Section id="instructions" title="Instructions">
        {diff != null ? (
          <PartText change={byPart("instructions")} label="Instructions" current={c.instructions} />
        ) : (
          <pre tabIndex={0} aria-label="Instructions" className={preClass}>
            {c.instructions}
          </pre>
        )}
      </Section>
      <Section id="skills" title="Skills" description={`${c.skills.length} ${c.skills.length === 1 ? "skill" : "skills"} the agent gets.`}>
        <div className="space-y-4">
          {c.skills.map((s) => (
            <SkillCard
              key={s.name}
              s={s}
              comparing={diff != null}
              changes={diff ? diff.changes.filter((x) => x.part === `skill:${s.name}` || x.part.startsWith(`${`skill:${s.name}`}:`)) : null}
            />
          ))}
        </div>
      </Section>
      <Section id="skill-refs" title="Skills from libraries" description="What the agent reads from a skill library instead of carrying its own copy.">
        {unchanged("skill_refs",
          <>
            <DetailChanges changes={group("skill_refs")} />
            <ul className="space-y-2">
              {(c.skill_refs as TemplateSkillRefView[]).map((r, i) => (
                <li key={i} className="space-y-1 rounded-card border border-line bg-surface p-4 text-sm" data-testid="template-skill-ref">
                  <p className="font-medium text-ink">
                    {r.kind === "library" ? r.skill : r.name} <Badge tone="neutral">{r.kind === "library" ? "Library" : "Organization"}</Badge>{" "}
                    <Badge tone="neutral">{r.required ? "Required" : "Optional"}</Badge>
                  </p>
                  {r.kind === "library" ? (
                    <p className="break-all font-mono text-xs text-ink-2">
                      {r.address}
                      {r.ref ? ` · ${r.ref}` : ""}
                      {r.path ? ` · ${r.path}` : ""}
                    </p>
                  ) : null}
                  <p className="text-ink-2">{r.why}</p>
                  {r.kind === "library" ? <p className="text-xs text-muted">Comes from that library as it is when an agent is created; its updates are reviewed in the library.</p> : null}
                </li>
              ))}
            </ul>
          </>,
        )}
      </Section>
      <Section id="helpers" title="Helper files" description="Files the agent keeps in .aurora/helpers, byte for byte.">
        {unchanged("helpers",
          <div className="space-y-3">
            {c.helpers.map((f) =>
              diff != null && f.text != null ? (
                <div key={f.path} className="space-y-1">
                  <FileHeading f={f} />
                  <PartText change={byPart(`helper:${f.path}`)} label={`Helper file ${f.path}`} current={f.text} />
                </div>
              ) : (
                <TextFile key={f.path} f={f} label={`Helper file ${f.path}`} />
              ),
            )}
          </div>,
        )}
      </Section>
      <Section id="memories" title="Memories" description="What the agent starts out knowing.">
        {unchanged("memories",
          <>
            <DetailChanges changes={group("memories").filter((x) => x.kind === "removed")} />
            <ul className="space-y-2">
              {c.memories.map((m) => (
                <li key={m.key} className="space-y-1 rounded-card border border-line bg-surface p-4" data-testid="template-memory">
                  {m.locked ? (
                    <p className="text-sm text-ink-2">
                      <Badge tone="neutral">Locked</Badge> Locked: {agent} can read it but not change it.
                    </p>
                  ) : null}
                  <p className="font-mono text-xs text-muted">{m.key}</p>
                  {diff != null ? (
                    <PartText change={byPart(`memory:${m.key}`)} label={`Memory ${m.key}`} current={m.content} />
                  ) : (
                    <pre aria-label={`Memory ${m.key}`} className="whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 text-sm text-ink [overflow-wrap:anywhere]">
                      {m.content}
                    </pre>
                  )}
                </li>
              ))}
            </ul>
          </>,
        )}
      </Section>
      <Section id="automations" title="Automations" description={`What ${agent} does on its own.`}>
        {unchanged("automations",
          <>
            <DetailChanges changes={group("automations")} />
            <ul className="space-y-2">
              {(c.automations as TemplateAutomationView[]).map((a, i) => (
                <li key={i} className="space-y-1 rounded-card border border-line bg-surface p-4 text-sm">
                  <p className="font-medium text-ink">
                    {a.name} <Badge tone="neutral">{a.kind === "schedule" ? "Schedule" : "Webhook"}</Badge>
                  </p>
                  {a.kind === "schedule" ? (
                    <>
                      <p className="whitespace-pre-wrap text-ink-2 [overflow-wrap:anywhere]">{a.prompt}</p>
                      <TechnicalDetails title="Schedule">
                        <span className="font-mono">{a.cron}</span>
                      </TechnicalDetails>
                    </>
                  ) : (
                    <p className="text-ink-2">
                      {a.instructions} {a.verification ? VERIFICATION_WORDS[a.verification] ?? "" : null}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          </>,
        )}
      </Section>
      <Section id="apps" title="Apps" description={`What ${agent} may reach, and how much.`}>
        {unchanged("apps",
          <>
            <DetailChanges changes={group("apps")} />
            <ul className="space-y-2">
              {(c.apps as TemplateAppEntryView[]).map((a, i) => (
                <li key={i} className="space-y-1 rounded-card border border-line bg-surface p-4 text-sm">
                  <p className="font-medium text-ink">
                    {a.app ? nameOf(a.app) : (a.any_of ?? []).map(nameOf).join(" or ")} <Badge tone="neutral">{a.required ? "Required" : "Optional"}</Badge>
                  </p>
                  <p className="text-ink-2">
                    {a.tools
                      ? `Custom access: may use ${(a.tools.allow ?? []).join(", ") || "nothing"}${(a.tools.ask ?? []).length ? `; asks before ${(a.tools.ask ?? []).join(", ")}` : ""}.`
                      : `${LEVEL_WORDS[a.level] ?? a.level} access.`}
                  </p>
                  <p className="text-xs text-muted">{a.why}</p>
                </li>
              ))}
            </ul>
          </>,
        )}
      </Section>
      <Section id="inputs" title="Inputs" description="What the person creating an agent from this template is asked.">
        {unchanged("inputs",
          <>
            <DetailChanges changes={group("inputs")} />
            <ul className="space-y-2">
              {(c.inputs as TemplateInputView[]).map((i) => (
                <li key={i.id} className="rounded-card border border-line bg-surface p-4 text-sm">
                  <p className="font-medium text-ink">
                    {i.label} <span className="font-mono text-xs text-muted">{i.id}</span> <Badge tone="neutral">{i.required ? "Required" : "Optional"}</Badge>
                  </p>
                  <p className="text-ink-2">
                    {i.kind === "choice" ? `One of ${(i.options ?? []).join(", ")}.` : "A line of text."}
                    {i.default ? ` Default: ${i.default}.` : ""}
                  </p>
                  {i.help ? <p className="text-xs text-muted">{i.help}</p> : null}
                </li>
              ))}
            </ul>
          </>,
        )}
      </Section>
    </>
  );
}

/** "About this template": the README as its source text, with **Preview** rendering it as people will see it (C-1116). */
function AboutThisTemplate({ readme, change, comparing }: { readme: string; change: TemplateChange | null; comparing: boolean }) {
  const [preview, setPreview] = useState(false);
  return (
    <Section
      id="about"
      title="About this template"
      description="The README.md the template's authors wrote, as its source text."
      actions={
        <Button size="sm" variant="outline" aria-pressed={preview} onClick={() => setPreview(!preview)}>
          {preview ? "Source text" : "Preview"}
        </Button>
      }
    >
      {preview ? (
        <TemplateReadme source={readme} />
      ) : comparing ? (
        <PartText change={change} label="About this template" current={readme} ariaLabel="README.md source text" />
      ) : (
        <pre aria-label="README.md source text" className={preClass}>
          {readme}
        </pre>
      )}
    </Section>
  );
}

/** The review page's body (C-1114): a short what-changed block, the version with each part's diff
 * inside its own section, and Approve / Reject. */
export function TemplateVersionReview({ templateId, version }: { templateId: string; version: string }) {
  const q = useTemplateVersion(templateId, version);
  const list = useAdminTemplates();
  const approve = useApproveTemplateVersion(templateId, version);
  const reject = useRejectTemplateVersion(templateId, version);
  const appName = useAppName();
  const [confirmReject, setConfirmReject] = useState(false);
  const d = q.data;
  const current = list.data?.items.find((t) => t.template_id === templateId)?.current_version ?? null;
  const firstVersion = list.data != null && current == null;
  const offered = useTemplateVersion(current && current !== version ? templateId : undefined, current ?? undefined);
  const diff = offered.data?.content && d?.content ? diffTemplateContent(offered.data.content, d.content, appName) : null;
  const stale = approve.error instanceof ApiError && approve.error.status === 409;
  const title = d ? `${d.content?.name ?? templateId} ${version}` : `${templateId} ${version}`;
  return (
    <div className="space-y-8">
      <PageHeader
        title={title}
        description={d?.content?.summary}
        breadcrumb={[{ label: "What agents may do", href: adminRoutes.agentPolicy("templates") }, { label: templateId }]}
        actions={d ? <Badge tone={d.status === "approved" ? "accent" : "neutral"}>{versionStatusWords(d.status)}</Badge> : null}
      />
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {d && !d.content ? <EmptyState title="Nothing to review">{<p className="text-sm text-ink-2">This version has no content to show.</p>}</EmptyState> : null}
      {d?.content ? (
        <>
          {stale ? (
            <Notice tone="pending" role="alert">
              This version changed while you were reading, so the review was reloaded. Check it again, then approve.{" "}
              <Button type="button" size="sm" variant="outline" onClick={() => void q.refetch()}>
                Reload the review
              </Button>
            </Notice>
          ) : approve.error || reject.error ? (
            <ErrorNotice error={approve.error ?? reject.error} />
          ) : null}
          {d.status === "pending_approval" && current && current !== version ? (
            <Notice role="status">People keep using {current} until you approve this one.</Notice>
          ) : null}
          {firstVersion ? <Notice role="status">This is the first version: nothing to compare with.</Notice> : null}
          {diff ? <WhatChanged diff={diff} baseVersion={current!} /> : null}
          <Content d={d} diff={diff} />
          {d.status === "pending_approval" ? (
            <div className="flex flex-wrap items-center justify-end gap-2" data-testid="template-decision">
              <Button type="button" variant="outline" disabled={reject.isPending || approve.isPending} onClick={() => setConfirmReject(true)}>
                Reject
              </Button>
              <Button
                type="button"
                variant="approve"
                disabled={approve.isPending || !d.content_sha256}
                onClick={() => d.content_sha256 && approve.mutate(d.content_sha256, { onError: () => undefined })}
              >
                Approve
              </Button>
            </div>
          ) : null}
          <ConfirmDialog
            open={confirmReject}
            onOpenChange={setConfirmReject}
            title={`Reject ${d.content?.name ?? templateId} ${version}?`}
            description="Aurora no longer offers this version, and it never reads this version string from the repository again."
            confirmLabel="Reject"
            danger
            busy={reject.isPending}
            onConfirm={() => reject.mutate(undefined, { onSuccess: () => setConfirmReject(false) })}
          />
        </>
      ) : null}
    </div>
  );
}
