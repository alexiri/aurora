"use client";

import { useState, type ReactNode } from "react";

import {
  SettingsSectionForm,
  type SettingControlCtx,
  type SettingsGroup,
} from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { FormSection } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { SettingField, settingControlId } from "@/components/ui/setting-field";
import { useSettingsSection, type SettingsSection } from "@/lib/api/settings";

/*
 * The operational Admin › Settings pages (SET-5, C-782): agents & sandboxes, approvals, retention, skill
 * libraries, directory sync, previews, terminal and limits. Each page is self-contained — it fetches its own
 * section and renders its own header — so it can stand alone or be embedded in a topic page later. There is no
 * sidebar link per section: they are reached from the Settings index.
 */

function when(iso: string | null | undefined): string {
  return iso ? new Date(iso).toLocaleString() : "never";
}

export interface OperationalPage {
  groups: SettingsGroup[];
}

export const AGENTS_PAGE: OperationalPage = {
  groups: [
    {
      title: "Defaults for new agents",
      description: "What a workspace or agent created from now on starts with. Existing ones keep what they have.",
      keys: ["agents.default_name", "agents.default_brief", "hosted.default_timezone"],
    },
    {
      title: "Hosted agents",
      description:
        "Hosted agents run in sandboxes on the runners. The harness list can only name harnesses a registered runner reports.",
      keys: ["hosted.enabled", "hosted.default_harness", "hosted.enabled_harnesses"],
    },
    {
      title: "Deadlines",
      description:
        "How long a sandbox may idle, stay awake for background work, or spend on one turn. A change applies to turns and starts begun afterwards.",
      keys: [
        "hosted.idle_stop_secs",
        "hosted.background_cap_secs",
        "hosted.turn_ceiling_secs",
        "hosted.turn_stall_secs",
        "hosted.sandbox_ready_timeout_secs",
      ],
    },
  ],
};

export const APPROVALS_PAGE: OperationalPage = {
  groups: [
    {
      title: "Approvals",
      description:
        "How long a held request waits for a decision. The gateway and the API read these for every hold recorded from now on.",
      keys: ["approvals.timeout_secs", "approvals.max_timeout_secs", "approvals.email_notifications"],
    },
  ],
};

export const RETENTION_PAGE: OperationalPage = {
  groups: [
    {
      title: "Logs and conversations",
      description:
        "Lowering a ceiling clamps every organization above it, from the next daily sweep. Their stored values stay and apply again if the ceiling is raised.",
      keys: ["retention.request_log_days", "retention.conversation_days_max"],
    },
    {
      title: "Attachments",
      description: "Uploaded files: the default quota of an organization that set none, and the ceilings above which none may go.",
      keys: ["attachments.default_org_quota_mib", "attachments.org_quota_mib_max", "attachments.days_max"],
    },
  ],
};

export const SKILLS_PAGE: OperationalPage = {
  groups: [
    {
      title: "Git skill libraries",
      description: "Skill libraries organizations register from git. Read at each refresh and fetch.",
      keys: ["skills.library.enabled", "skills.library.refresh_interval_secs", "skills.library.bytes_per_agent"],
    },
  ],
};

export const SCIM_PAGE: OperationalPage = {
  groups: [
    {
      title: "Directory sync (SCIM)",
      description: "Provisioning of users and groups from an organization's identity provider.",
      keys: ["scim.enabled", "scim.allowed_email_domains", "scim.rate_per_minute", "scim.token_max_days"],
    },
  ],
};

export const PREVIEWS_PAGE: OperationalPage = {
  groups: [
    {
      title: "Port previews",
      description: "Limits for shared previews of ports in sandboxes. Read each time a preview is used.",
      keys: [
        "previews.session_ttl_secs",
        "previews.session_idle_secs",
        "previews.awake_cap_secs",
        "previews.shares_per_agent",
        "previews.shares_per_org",
        "previews.bytes_per_share_day",
      ],
    },
  ],
};

export const TERMINAL_PAGE: OperationalPage = {
  groups: [
    {
      title: "Web terminal",
      description:
        "An interactive shell in a sandbox from the dashboard. Turning it off closes new sessions at once; open ones end at their next wake check.",
      keys: ["terminal.enabled", "terminal.awake_cap_secs", "terminal.wake_window_secs", "terminal.wake_wait_secs"],
    },
  ],
};

export const LIMITS_PAGE: OperationalPage = {
  groups: [
    {
      title: "API rate limits",
      description:
        "Token buckets of the management API. A change applies from the next request; nobody's remaining budget is reset.",
      keys: ["ratelimit.api.multiplier", "ratelimit.api.org_per_minute", "ratelimit.api.classes"],
    },
    {
      title: "Gateway",
      description: "Where agents may connect and how much of a request the gateway reads. Applies to requests begun afterwards.",
      keys: ["gateway.allowed_private", "gateway.condition_body_bytes", "gateway.upstream_header_timeout_secs"],
    },
  ],
};

/** The shared page: header, optional notices, the form, optional content beneath. */
function OperationalView({
  sectionId,
  page,
  notice,
  controls,
  below,
  bare = false,
}: {
  sectionId: string;
  page: OperationalPage;
  notice?: (s: SettingsSection) => ReactNode;
  controls?: Record<string, (ctx: SettingControlCtx) => ReactNode>;
  below?: (s: SettingsSection) => ReactNode;
  /** Part of a topic page's tab (P5-IA-3): no page header of its own, only the titled groups. */
  bare?: boolean;
}) {
  const q = useSettingsSection(sectionId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  return (
    <div className={bare ? "space-y-6" : "max-w-page space-y-6"}>
      {bare ? null : (
        <PageHeader
          breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: section.title }]}
          title={section.title}
          description={`${section.description} Last change: ${section.updated_by_label ? `${section.updated_by_label}, ` : ""}${when(section.updated_at)}.`}
        />
      )}
      {notice ? notice(section) : null}
      <SettingsSectionForm
        key={section.generation}
        section={section}
        groups={page.groups}
        controls={controls}
        onReload={() => void q.refetch()}
      />
      {below ? below(section) : null}
    </div>
  );
}

interface RunnerRow {
  id: string;
  name: string;
  sandbox_memory_mib: number | null;
  sandbox_cpus: number | null;
  sandbox_pids: number | null;
  home_gib: number | null;
  max_sandboxes: number | null;
  harnesses: string[] | null;
}

function runnersOf(section: SettingsSection): RunnerRow[] {
  const extra = section.extra as { runners?: RunnerRow[] } | null | undefined;
  return extra?.runners ?? [];
}

function cell(v: number | null | undefined): string {
  return v === null || v === undefined ? "—" : String(v);
}

/** The runners' sizes and limits, read-only (D-235(a)): they stay in Helm. */
function RunnersTable({ section }: { section: SettingsSection }) {
  const runners = runnersOf(section);
  return (
    <FormSection
      title="Runners and sandbox sizes"
      description={
        <>
          Reported by each registered runner. Set in Helm: <code className="font-mono">runner.sandbox_cpus</code>,{" "}
          <code className="font-mono">runner.sandbox_memory_mib</code>, <code className="font-mono">runner.sandbox_pids</code>,{" "}
          <code className="font-mono">runner.max_sandboxes</code>.
        </>
      }
    >
      {runners.length === 0 ? (
        <p className="text-sm text-ink-2">No runner has registered yet.</p>
      ) : (
        <DataTable caption="Runners and sandbox sizes">
          <thead>
            <tr>
              <Th>Runner</Th>
              <Th>Memory (MiB)</Th>
              <Th>CPUs</Th>
              <Th>Processes</Th>
              <Th>Home (GiB)</Th>
              <Th>Max sandboxes</Th>
              <Th>Harnesses</Th>
            </tr>
          </thead>
          <tbody>
            {runners.map((r) => (
              <tr key={r.id}>
                <Td className="font-medium">{r.name}</Td>
                <Td>{cell(r.sandbox_memory_mib)}</Td>
                <Td>{cell(r.sandbox_cpus)}</Td>
                <Td>{cell(r.sandbox_pids)}</Td>
                <Td>{cell(r.home_gib)}</Td>
                <Td>{cell(r.max_sandboxes)}</Td>
                <Td>{(r.harnesses ?? []).join(", ") || "—"}</Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      )}
    </FormSection>
  );
}

/** Admin › Settings › Agents & sandboxes (SET-5). */
export function AgentsSettingsView() {
  return <OperationalView sectionId="agents" page={AGENTS_PAGE} below={(s) => <RunnersTable section={s} />} />;
}

/** Admin › Settings › Approvals (SET-5). */
export function ApprovalsSettingsView() {
  return <OperationalView sectionId="approvals" page={APPROVALS_PAGE} />;
}

/** Administration › Agents › Defaults (C-967): the agents and approvals sections as one run of titled groups. */
export function AgentsDefaultsView() {
  return (
    <div className="space-y-8">
      <OperationalView bare sectionId="agents" page={AGENTS_PAGE} below={(s) => <RunnersTable section={s} />} />
      <OperationalView bare sectionId="approvals" page={APPROVALS_PAGE} />
    </div>
  );
}

/** Admin › Settings › Retention (SET-5). */
export function RetentionSettingsView() {
  return <OperationalView sectionId="retention" page={RETENTION_PAGE} />;
}

/** Admin › Settings › Skill libraries (SET-5). */
export function SkillsSettingsView({ bare = false }: { bare?: boolean }) {
  return <OperationalView bare={bare} sectionId="skills" page={SKILLS_PAGE} />;
}

/** Admin › Settings › Previews (SET-5). */
export function PreviewsSettingsView({ bare = false }: { bare?: boolean }) {
  return (
    <OperationalView
      bare={bare}
      sectionId="previews"
      page={PREVIEWS_PAGE}
      notice={() => (
        <Notice tone="neutral" role="status" className="max-w-3xl">
          Whether previews are on and their domain are set in Helm (<code className="font-mono">relay.enabled</code>,{" "}
          <code className="font-mono">relay.domain</code>); {bare ? "these are" : "this page sets"} their limits.
        </Notice>
      )}
    />
  );
}

function availableOf(section: SettingsSection): boolean {
  return (section.extra as { available?: boolean } | null | undefined)?.available === true;
}

/** Admin › Settings › Directory sync (SET-5). */
export function ScimSettingsView() {
  return (
    <OperationalView
      sectionId="scim"
      page={SCIM_PAGE}
      notice={(s) =>
        availableOf(s) ? null : (
          <Notice tone="pending" role="status" className="max-w-3xl">
            This deployment does not route <code className="font-mono">/scim/v2</code> to Aurora, so directory sync cannot be turned
            on here. Set <code className="font-mono">scim.enabled: true</code> in your Helm values, then come back.
          </Notice>
        )
      }
    />
  );
}

/** Admin › Settings › Terminal (SET-5). */
export function TerminalSettingsView({ bare = false }: { bare?: boolean }) {
  return (
    <OperationalView
      bare={bare}
      sectionId="terminal"
      page={TERMINAL_PAGE}
      notice={(s) =>
        availableOf(s) ? null : (
          <Notice tone="pending" role="status" className="max-w-3xl">
            This deployment does not provide the terminal&apos;s wiring (the runner&apos;s stream listener and the network flows to it),
            so the terminal cannot be turned on here. Set <code className="font-mono">terminal.enabled: true</code> in your Helm
            values, then come back.
          </Notice>
        )
      }
    />
  );
}

/** Administration › Agents › Computers (C-967): the terminal and the previews as one run of titled groups. */
export function ComputersView() {
  return (
    <div className="space-y-8">
      <TerminalSettingsView bare />
      <PreviewsSettingsView bare />
    </div>
  );
}

/** Admin › Settings › Limits (SET-5): rate limits with the keyed class list, and the gateway's address and size limits. */
export function LimitsSettingsView() {
  return (
    <OperationalView
      sectionId="limits"
      page={LIMITS_PAGE}
      notice={() => (
        <Notice tone="danger" role="status" className="max-w-3xl">
          <strong>Private destinations.</strong> Every entry in the allowed-private list opens an address range to agents, apps
          and the gateway&apos;s own connections. Add only what they must reach; the confirmation lists what a change opens.
        </Notice>
      )}
      controls={{ "ratelimit.api.classes": (ctx) => <RateClassesControl ctx={ctx} /> }}
    />
  );
}

/** The rate classes of `05 §Conventions` an override may name. */
export const RATE_CLASSES = ["general", "write", "message", "upload", "simulate", "search", "export", "telemetry"] as const;

interface RateEntry {
  class: string;
  per_minute?: number;
  per_hour?: number;
  burst?: number;
}

/** The keyed list of rate-class overrides (C-782): `{ class, per_minute | per_hour, burst }` per class. */
export function RateClassesControl({ ctx }: { ctx: SettingControlCtx }) {
  const s = ctx.setting;
  const helm = (s.entries ?? []).filter((e) => e.source === "helm");
  const own = ctx.own as unknown as RateEntry[];
  const taken = new Set<string>([...helm.map((e) => e.id), ...own.map((e) => e.class)]);
  const free = RATE_CLASSES.filter((c) => !taken.has(c));
  const [pick, setPick] = useState<string>("");
  const cid = settingControlId(s.key);

  function commit(next: RateEntry[]) {
    if (next.length === 0) ctx.resetValue(s.key);
    else ctx.setValue(s.key, next);
  }
  function patch(i: number, p: Partial<RateEntry>) {
    commit(own.map((e, j) => (j === i ? { ...e, ...p } : e)));
  }
  const num = (v: string): number | undefined => (v === "" ? undefined : Number(v));

  return (
    <SettingField
      id={s.key}
      label="Rate classes"
      hint="Override the limit of one request class. The default of each class is in the API reference; the multiplier above scales it."
      source={own.length > 0 ? "dashboard" : "default"}
      error={ctx.error ?? ctx.entryErrors.find(([, m]) => m)?.[1]}
    >
      <div id={cid} className="space-y-2">
        <ul aria-label="Rate class overrides" className="space-y-2">
          {helm.map((e) => (
            <li key={`h-${e.id}`} className="flex flex-wrap items-center gap-2 rounded-control border border-line p-2 text-sm">
              <span className="font-medium text-ink">{e.id}</span>
              <code className="font-mono text-xs text-ink-2">{JSON.stringify(e.value)}</code>
              <Badge tone="pending">Managed by Helm</Badge>
            </li>
          ))}
          {own.map((e, i) => {
            const unit = e.per_hour !== undefined ? "per_hour" : "per_minute";
            return (
              <li key={e.class} className="flex flex-wrap items-center gap-2 rounded-control border border-line p-2 text-sm">
                <span className="min-w-20 font-medium text-ink">{e.class}</span>
                <input
                  aria-label={`${e.class}: limit`}
                  type="number"
                  min={1}
                  className={`${inputClass} max-w-28`}
                  value={String(e[unit] ?? "")}
                  onChange={(ev) => patch(i, { per_minute: undefined, per_hour: undefined, [unit]: num(ev.target.value) })}
                />
                <select
                  aria-label={`${e.class}: unit`}
                  className={`${inputClass} max-w-32`}
                  value={unit}
                  onChange={(ev) => {
                    const v = e[unit];
                    patch(i, ev.target.value === "per_hour" ? { per_minute: undefined, per_hour: v } : { per_hour: undefined, per_minute: v });
                  }}
                >
                  <option value="per_minute">per minute</option>
                  <option value="per_hour">per hour</option>
                </select>
                <label className="flex items-center gap-1 text-xs text-ink-2">
                  Burst
                  <input
                    aria-label={`${e.class}: burst`}
                    type="number"
                    min={1}
                    className={`${inputClass} max-w-24`}
                    value={String(e.burst ?? "")}
                    onChange={(ev) => patch(i, { burst: num(ev.target.value) })}
                  />
                </label>
                <Button type="button" variant="ghost" size="sm" onClick={() => commit(own.filter((_, j) => j !== i))}>
                  Remove
                </Button>
              </li>
            );
          })}
          {helm.length + own.length === 0 ? <li className="text-sm text-ink-2">No class is overridden.</li> : null}
        </ul>
        {free.length > 0 ? (
          <div className="flex flex-wrap items-center gap-2">
            <select aria-label="Class to override" className={`${inputClass} max-w-40`} value={pick} onChange={(e) => setPick(e.target.value)}>
              <option value="">Choose a class…</option>
              {free.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={pick === ""}
              onClick={() => {
                commit([...own, { class: pick, per_minute: 60, burst: 10 }]);
                setPick("");
              }}
            >
              Add override
            </Button>
          </div>
        ) : null}
      </div>
    </SettingField>
  );
}
