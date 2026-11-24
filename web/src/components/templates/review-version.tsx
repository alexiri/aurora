"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Lines } from "@/components/ui/lines";
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
  type TemplateContentView,
  type TemplateVersionDetail,
} from "@/lib/api/agent-templates";
import { adminRoutes } from "@/lib/routes";
import { formatCount } from "@/lib/format-date";

import { TemplateReadme } from "./template-readme";
import { useAppName, versionStatusWords } from "./words";

// The review of one version (C-1114): everything of `TemplateContentView` in full, all of it marked
// as text from the source at the commit, the diff against the offered version, and the decision.
// Every part is data — the review only ever reads.

// The C-1104 shapes passed through as stored (`content_json`): the review reads them as data.
interface InputView {
  id: string;
  label: string;
  help: string | null;
  kind: string;
  required: boolean;
  pattern: string | null;
  max_length: number | null;
  options: string[] | null;
  default: string | null;
}
interface AppToolsView {
  allow: string[];
  ask: string[];
}
interface AppEntryView {
  app?: string;
  any_of?: string[];
  required: boolean;
  level: string;
  tools?: AppToolsView | null;
  why: string;
}
interface SkillRefView {
  kind: string;
  address?: string;
  ref?: string | null;
  path?: string | null;
  skill?: string;
  name?: string;
  required: boolean;
  why: string;
}
interface AutomationView {
  kind: string;
  name: string;
  prompt?: string;
  cron?: string;
  instructions?: string;
  verification?: string;
}

const LEVEL_WORDS: Record<string, string> = { read: "Read", ask: "Ask", act: "Act" };
const VERIFICATION_WORDS: Record<string, string> = {
  signed: "Verified with a signature.",
  password: "Verified with a password.",
  none: "No verification.",
};

/** A text file in full; executable is said in words. Nothing is ever run. */
function TextFile({ f, label }: { f: FileView; label: string }) {
  return (
    <div className="space-y-1">
      <p className="text-sm font-semibold text-ink">
        <span className="font-mono">{f.path}</span>{" "}
        <span className="font-normal text-muted">
          ({formatCount(f.size)} chars{f.executable ? ", executable" : ""})
        </span>
      </p>
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

/** One text part of the diff against the offered version: only the parts that differ. */
function diffParts(base: TemplateContentView, next: TemplateContentView): { label: string; old: string; new: string }[] {
  const parts: { label: string; old: string; new: string }[] = [];
  if (base.instructions !== next.instructions) parts.push({ label: "Instructions", old: base.instructions, new: next.instructions });
  const names = new Set([...base.skills.map((s) => s.name), ...next.skills.map((s) => s.name)]);
  for (const name of names) {
    const b = base.skills.find((s) => s.name === name);
    const n = next.skills.find((s) => s.name === name);
    if (b?.body !== n?.body) parts.push({ label: `Skill ${name}`, old: b?.body ?? "", new: n?.body ?? "" });
    const paths = new Set([...(b?.files ?? []).map((f) => f.path), ...(n?.files ?? []).map((f) => f.path)]);
    for (const p of paths) {
      const bf = b?.files.find((f) => f.path === p)?.text ?? null;
      const nf = n?.files.find((f) => f.path === p)?.text ?? null;
      if (bf !== nf) parts.push({ label: `Skill ${name} · ${p}`, old: bf ?? "", new: nf ?? "" });
    }
  }
  const helperPaths = new Set([...base.helpers.map((f) => f.path), ...next.helpers.map((f) => f.path)]);
  for (const p of helperPaths) {
    const b = base.helpers.find((f) => f.path === p)?.text ?? null;
    const n = next.helpers.find((f) => f.path === p)?.text ?? null;
    if (b !== n) parts.push({ label: `Helper file ${p}`, old: b ?? "", new: n ?? "" });
  }
  const keys = new Set([...base.memories.map((m) => m.key), ...next.memories.map((m) => m.key)]);
  for (const k of keys) {
    const b = base.memories.find((m) => m.key === k)?.content ?? "";
    const n = next.memories.find((m) => m.key === k)?.content ?? "";
    if (b !== n) parts.push({ label: `Memory ${k}`, old: b, new: n });
  }
  return parts;
}

/** Every part of one version, in full (C-1114). `<Agent>` is the agent the template would create. */
function Content({ d }: { d: TemplateVersionDetail }) {
  const c = d.content!;
  const agent = c.agent.name;
  const nameOf = useAppName();
  return (
    <>
      <p className="text-sm text-ink-2">
        Text from <span className="font-medium text-ink">{d.source.name}</span> at <span className="font-mono">{d.commit_sha.slice(0, 7)}</span>. The package matches the repository at{" "}
        <span className="font-mono">{d.commit_sha.slice(0, 7)}</span>.
      </p>
      <AboutThisTemplate readme={c.readme} />
      <Section id="instructions" title="Instructions">
        <pre tabIndex={0} aria-label="Instructions" className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 text-sm text-ink [overflow-wrap:anywhere]">
          {c.instructions}
        </pre>
      </Section>
      <Section id="skills" title="Skills" description={`${c.skills.length} ${c.skills.length === 1 ? "skill" : "skills"} the agent gets.`}>
        <div className="space-y-4">
          {c.skills.map((s) => (
            <section key={s.name} aria-label={`Skill ${s.name}`} className="space-y-2 rounded-card border border-line bg-surface p-4">
              <p className="font-medium text-ink">{s.name}</p>
              <p className="text-sm text-ink-2">Use it when: {s.use_it_when}</p>
              <pre tabIndex={0} aria-label={`Skill ${s.name} body`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
                {s.body}
              </pre>
              {s.files.map((f) => (
                <TextFile key={f.path} f={f} label={`File ${f.path} of skill ${s.name}`} />
              ))}
            </section>
          ))}
        </div>
      </Section>
      <Section id="skill-refs" title="Skills from libraries" description="What the agent reads from a skill library instead of carrying its own copy.">
        <ul className="space-y-2">
          {(c.skill_refs as SkillRefView[]).map((r, i) => (
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
      </Section>
      <Section id="helpers" title="Helper files" description="Files the agent keeps in .aurora/helpers, byte for byte.">
        <div className="space-y-3">
          {c.helpers.map((f) => (
            <TextFile key={f.path} f={f} label={`Helper file ${f.path}`} />
          ))}
        </div>
      </Section>
      <Section id="memories" title="Memories" description="What the agent starts out knowing.">
        <ul className="space-y-2">
          {c.memories.map((m) => (
            <li key={m.key} className="space-y-1 rounded-card border border-line bg-surface p-4" data-testid="template-memory">
              {m.locked ? (
                <p className="text-sm text-ink-2">
                  <Badge tone="neutral">Locked</Badge> Locked: {agent} can read it but not change it.
                </p>
              ) : null}
              <p className="font-mono text-xs text-muted">{m.key}</p>
              <pre aria-label={`Memory ${m.key}`} className="whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 text-sm text-ink [overflow-wrap:anywhere]">
                {m.content}
              </pre>
            </li>
          ))}
        </ul>
      </Section>
      <Section id="automations" title="Automations" description={`What ${agent} does on its own.`}>
        <ul className="space-y-2">
          {(c.automations as AutomationView[]).map((a, i) => (
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
      </Section>
      <Section id="apps" title="Apps" description={`What ${agent} may reach, and how much.`}>
        <ul className="space-y-2">
          {(c.apps as AppEntryView[]).map((a, i) => (
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
      </Section>
      <Section id="inputs" title="Inputs" description="What the person creating an agent from this template is asked.">
        <ul className="space-y-2">
          {(c.inputs as InputView[]).map((i) => (
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
      </Section>
    </>
  );
}

/** "About this template": the README as its source text, with **Preview** rendering it as people will see it (C-1116). */
function AboutThisTemplate({ readme }: { readme: string }) {
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
      ) : (
        <pre aria-label="README.md source text" className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 text-sm text-ink [overflow-wrap:anywhere]">
          {readme}
        </pre>
      )}
    </Section>
  );
}

/** The review page's body (C-1114): the version in full, the diff against the offered version, Approve / Reject. */
export function TemplateVersionReview({ templateId, version }: { templateId: string; version: string }) {
  const q = useTemplateVersion(templateId, version);
  const list = useAdminTemplates();
  const approve = useApproveTemplateVersion(templateId, version);
  const reject = useRejectTemplateVersion(templateId, version);
  const [confirmReject, setConfirmReject] = useState(false);
  const d = q.data;
  const current = list.data?.items.find((t) => t.template_id === templateId)?.current_version ?? null;
  const offered = useTemplateVersion(current && current !== version ? templateId : undefined, current ?? undefined);
  const stale = approve.error instanceof ApiError && approve.error.status === 409;
  const decidable = d?.status === "pending_approval" && !!d.content;
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
          <Content d={d} />
          {offered.data?.content ? <DiffAgainstOffered offered={offered.data.content} next={d.content} offeredVersion={current!} /> : null}
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

/** The diff of every text part against the offered version (C-1114): whole lines, nothing summarized. */
function DiffAgainstOffered({ offered, next, offeredVersion }: { offered: TemplateContentView; next: TemplateContentView; offeredVersion: string }) {
  const parts = diffParts(offered, next);
  return (
    <Section id="diff" title={`Changes against ${offeredVersion}`} description={parts.length === 0 ? "The content matches the offered version." : "Every line that differs, old against new."}>
      {parts.map((p) => (
        <section key={p.label} aria-label={`Changes to ${p.label}`} className="space-y-1">
          <h3 className="text-sm font-semibold text-ink">{p.label}</h3>
          <Lines old={p.old} next={p.new} label={`Diff of ${p.label}`} />
        </section>
      ))}
    </Section>
  );
}
