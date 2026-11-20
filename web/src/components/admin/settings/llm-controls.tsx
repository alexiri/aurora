"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import { EndpointDialog, HelmEndpointDialog, PROVIDER_LABEL } from "@/components/admin/settings/llm-endpoint-dialog";
import type { SettingControlCtx } from "@/components/admin/settings/settings-section-form";
import { harnessNames } from "@/components/credentials/llm-key-modes";
import { Badge } from "@/components/ui/badge";
import { LockPill } from "@/components/ui/lock-pill";
import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow } from "@/components/ui/section";
import { SettingField, settingControlId } from "@/components/ui/setting-field";
import { StatusLine } from "@/components/ui/status-line";
import { useLlmEndpointWarnings, type EndpointWarnings } from "@/lib/api/llm-endpoint-warnings";
import { ROLE_LABEL, ROLES } from "@/lib/model-roles";
import { adminRoutes } from "@/lib/routes";

export const ENABLED = "llm.enabled_providers";
export const PREFERENCE = "llm.provider_preference";
export const ENDPOINTS = "llm.endpoints";

const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const label = (p: string): string => PROVIDER_LABEL[p] ?? p;

/** The enabled providers now: the pending value, else the effective one. */
function enabledNow(ctx: SettingControlCtx, effective: unknown): string[] {
  return strs(ENABLED in ctx.values ? ctx.values[ENABLED] : effective);
}

/** The order hosted agents pick a provider in: the preference restricted to enabled providers, then the rest. */
export function orderedPreference(pref: string[], enabled: string[]): string[] {
  const out = pref.filter((p) => enabled.includes(p));
  for (const p of enabled) if (!out.includes(p)) out.push(p);
  return out;
}

/** The enabled providers as a checkbox group, or the locked value when Helm sets the list (SET-3). */
export function ProvidersControl({
  ctx,
  available,
  preference,
}: {
  ctx: SettingControlCtx;
  available: string[];
  /** The preference setting, to keep it consistent when a provider is switched off. */
  preference: { value: unknown; locked: boolean };
}) {
  const s = ctx.setting;
  const enabled = strs(ctx.current);
  const cid = settingControlId(s.key);
  if (s.locked) {
    return (
      <SettingField
        id={s.key}
        label="Enabled providers"
        source="helm"
        helmKey={s.helm_key}
        lockedValue={enabled.length ? enabled.map(label).join(", ") : "None"}
      />
    );
  }
  function toggle(p: string, on: boolean) {
    const next = on ? [...enabled, p] : enabled.filter((x) => x !== p);
    ctx.setValue(ENABLED, next);
    // A preference may only name enabled providers: switching one off removes it from the order.
    if (!on && !preference.locked) {
      const pref = strs(PREFERENCE in ctx.values ? ctx.values[PREFERENCE] : preference.value);
      if (pref.includes(p)) ctx.setValue(PREFERENCE, pref.filter((x) => x !== p));
    }
  }
  return (
    <SettingField
      id={s.key}
      label="Enabled providers"
      hint="Only providers this release can run are listed."
      source={ctx.hasValue ? "dashboard" : "default"}
      defaultLabel={strs(s.default).map(label).join(", ")}
      onReset={() => ctx.resetValue(s.key)}
      error={ctx.error}
    >
      <div id={cid} role="group" aria-label="Enabled providers" className="space-y-2">
        {available.map((p) => (
          <CheckboxField key={p} id={`prov-${p}`} label={label(p)} checked={enabled.includes(p)} onChange={(on) => toggle(p, on)} />
        ))}
      </div>
    </SettingField>
  );
}

/** The provider preference as a reorderable list (buttons, so the keyboard reaches every move). */
export function PreferenceControl({ ctx, enabledEffective }: { ctx: SettingControlCtx; enabledEffective: unknown }) {
  const s = ctx.setting;
  const order = orderedPreference(strs(ctx.current), enabledNow(ctx, enabledEffective));
  const cid = settingControlId(s.key);
  if (s.locked) {
    return (
      <SettingField
        id={s.key}
        label="Preference order"
        source="helm"
        helmKey={s.helm_key}
        lockedValue={order.length ? order.map((p, i) => `${i + 1}. ${label(p)}`).join(" · ") : "None"}
      />
    );
  }
  function move(i: number, by: -1 | 1) {
    const next = [...order];
    const j = i + by;
    if (j < 0 || j >= next.length) return;
    [next[i], next[j]] = [next[j]!, next[i]!];
    ctx.setValue(PREFERENCE, next);
  }
  return (
    <SettingField
      id={s.key}
      label="Preference order"
      hint="Hosted agents use the first provider in this list that has a usable key."
      source={ctx.hasValue ? "dashboard" : "default"}
      defaultLabel={strs(s.default).map(label).join(", ")}
      onReset={() => ctx.resetValue(s.key)}
      error={ctx.error}
    >
      <ol id={cid} aria-label="Preference order" className="divide-y divide-line rounded-control border border-line">
        {order.length === 0 ? <li className="p-3 text-sm text-ink-2">No provider is enabled.</li> : null}
        {order.map((p, i) => (
          <li key={p} className="flex flex-wrap items-center gap-2 p-2 sm:px-3">
            <span className="w-6 text-sm text-muted">{i + 1}.</span>
            <span className="min-w-0 flex-1 text-sm text-ink">{label(p)}</span>
            <Button type="button" size="sm" variant="outline" aria-label={`Move ${label(p)} up`} disabled={i === 0} onClick={() => move(i, -1)}>
              Up
            </Button>
            <Button type="button" size="sm" variant="outline" aria-label={`Move ${label(p)} down`} disabled={i === order.length - 1} onClick={() => move(i, 1)}>
              Down
            </Button>
          </li>
        ))}
      </ol>
    </SettingField>
  );
}

function hostOf(url: unknown): string {
  return typeof url === "string" ? url : "";
}

/** The harnesses an endpoint serves as one plain line, or the notice that it serves none (D-260(a), C-811). */
function HarnessLine({ harnesses }: { harnesses: string[] | undefined }) {
  if (harnesses === undefined) return null;
  if (harnesses.length === 0) {
    return (
      <Notice tone="pending" role="status" className="max-w-xl text-xs">
        No enabled harness speaks this API. Put a LiteLLM endpoint in front to translate.
      </Notice>
    );
  }
  return (
    <p className="text-xs text-ink-2" aria-label="Harnesses this endpoint serves">
      Works with {harnessNames(harnesses)}
    </p>
  );
}

/** An endpoint's kind as plain text ("LiteLLM gateway"), not a chip. */
const KIND_LABEL: Record<string, string> = { litellm: "LiteLLM gateway" };
const kindLabel = (p: string): string => KIND_LABEL[p] ?? label(p);

/** "Large, Medium and Small" */
function roleList(roles: string[]): string {
  const names = roles.map((r) => ROLE_LABEL[r as keyof typeof ROLE_LABEL] ?? r);
  return names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : (names[0] ?? "");
}

/**
 * (P5-POLISH-10, C-1034, D-385) The endpoint's role warning, once, with its fix. A dashboard endpoint gets the amber line and
 * **Set roles** (opens its editor); a Helm one cannot be changed here, so the line is neutral and says where.
 */
function RoleWarningLine({ id, helm, warnings, onSetRoles }: { id: string; helm: boolean; warnings: EndpointWarnings[]; onSetRoles?: () => void }) {
  const w = warnings.find((x) => x.endpoint_id === id);
  if (!w || w.roles.length === 0) return null;
  const roles = roleList(w.roles.map((r) => r.role));
  if (helm) {
    return (
      <div data-testid="endpoint-role-warning">
        <StatusLine tone="idle" className="text-xs">
          Agents may ask for a model this endpoint doesn&apos;t serve. Set the {roles} {w.roles.length === 1 ? "role" : "roles"} in the Helm values under{" "}
          <code className="font-mono">llm.endpoints.{id}.roles</code>.
        </StatusLine>
      </div>
    );
  }
  return (
    <div data-testid="endpoint-role-warning">
      <StatusLine
        tone="attention"
        className="text-xs"
        action={
          <Button type="button" size="sm" variant="outline" aria-label={`Set roles of ${id}`} onClick={onSetRoles}>
            Set roles
          </Button>
        }
      >
        Agents may ask for a model this endpoint doesn&apos;t serve: set the {roles} {w.roles.length === 1 ? "role" : "roles"}.
      </StatusLine>
    </div>
  );
}

/**
 * (phase 5, P5-MODELS-3, C-875) The roles an endpoint names, or "Roles from the provider" for Anthropic. An endpoint
 * that names none says nothing here: its warning is `RoleWarningLine`, from the server (C-1034, D-385).
 */
function RolesLine({ entry }: { entry: Record<string, unknown> }) {
  const roles = (entry.model_roles ?? {}) as Record<string, unknown>;
  const named = ROLES.filter((r) => typeof roles[r] === "string" && roles[r] !== "");
  if (named.length > 0) {
    return (
      <p className="break-words text-xs text-ink-2" data-testid="endpoint-roles">
        Roles: {named.map((r) => `${ROLE_LABEL[r]} ${String(roles[r])}`).join(" · ")}
      </p>
    );
  }
  if (entry.provider === "anthropic") return <p className="text-xs text-ink-2">Roles from the provider</p>;
  return null;
}

/** The endpoints: Helm entries read-only ("Managed by Helm"), dashboard entries editable and removable. */
export function EndpointsControl({
  ctx,
  available,
  enabledEffective,
  harnesses,
}: {
  ctx: SettingControlCtx;
  available: string[];
  enabledEffective: unknown;
  /** The enabled harnesses that speak each endpoint's API, by endpoint id (`extra.endpoint_harnesses`). */
  harnesses?: Record<string, string[]>;
}) {
  const s = ctx.setting;
  const helm = (s.entries ?? []).filter((e) => e.source === "helm");
  // C-813: the saved dashboard endpoint whose host the gateway would refuse (a private name nothing opens).
  const needsPrivate = (id: string): boolean =>
    (s.entries ?? []).some((e) => e.id === id && e.source === "dashboard" && e.warnings?.includes("needs_allowed_private"));
  const own = ctx.own;
  const helmIds = helm.map((e) => e.id);
  const warningsData = useLlmEndpointWarnings().data;
  const roleWarnings = Array.isArray(warningsData) ? warningsData : [];
  const enabled = enabledNow(ctx, enabledEffective);
  const providers = available.filter((p) => enabled.includes(p));
  const [editing, setEditing] = useState<{ index: number | null } | null>(null);
  const [viewing, setViewing] = useState<Record<string, unknown> | null>(null);
  const errorsOf = (id: string): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const [k, m] of ctx.entryErrors) {
      const rest = k.startsWith(`${ENDPOINTS}/${id}/`) ? k.slice(`${ENDPOINTS}/${id}/`.length) : null;
      if (rest) out[rest] = m;
    }
    return out;
  };
  const idOf = (e: Record<string, unknown>): string => (typeof e.id === "string" ? e.id : "");
  const editingEntry = editing && editing.index !== null ? own[editing.index]! : null;
  function save(e: Record<string, unknown>) {
    const next = [...own];
    if (editing?.index === null || editing === null) next.push(e);
    else next[editing.index] = e;
    ctx.setValue(ENDPOINTS, next);
    setEditing(null);
  }
  const rows: ReactNode[] = [];
  for (const e of helm) {
    const v = e.value as Record<string, unknown>;
    rows.push(
      <ListRow
        key={`h-${e.id}`}
        actions={
          <Button type="button" size="sm" variant="outline" aria-label={`View ${e.id}`} onClick={() => setViewing(v)}>
            View
          </Button>
        }
      >
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-ink">{e.id}</span>
            <LockPill owner="Set by the installation" />
          </div>
          <p className="text-xs text-ink-2">{kindLabel(String(v.provider ?? ""))}</p>
          {hostOf(v.base_url) ? <p className="break-all text-xs text-ink-2">{hostOf(v.base_url)}</p> : null}
          <HarnessLine harnesses={harnesses?.[e.id]} />
          <RolesLine entry={v} />
          <RoleWarningLine id={e.id} helm warnings={roleWarnings} />
        </div>
      </ListRow>,
    );
  }
  own.forEach((e, i) => {
    const id = idOf(e);
    const errs = errorsOf(id);
    const bad = Object.keys(errs).length > 0;
    rows.push(
      <ListRow
        key={`d-${id}-${i}`}
        actions={
          <>
            <Button type="button" size="sm" variant="outline" aria-label={`Edit ${id}`} onClick={() => setEditing({ index: i })}>
              Edit
            </Button>
            <Button type="button" size="sm" variant="danger" aria-label={`Remove ${id}`} onClick={() => ctx.setValue(ENDPOINTS, own.filter((_, j) => j !== i))}>
              Remove
            </Button>
          </>
        }
      >
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-ink">{id}</span>
            {/* At most one chip: a problem with the entry, else none. */}
            {bad ? (
              <Badge tone="danger">Needs attention</Badge>
            ) : needsPrivate(id) ? (
              <Link href={adminRoutes.sectionPage("limits", "gateway.allowed_private")} className="rounded-control focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
                <Badge tone="danger">Private address not allowed</Badge>
              </Link>
            ) : null}
          </div>
          <p className="text-xs text-ink-2">{kindLabel(String(e.provider ?? ""))}</p>
          {hostOf(e.base_url) ? <p className="break-all text-xs text-ink-2">{hostOf(e.base_url)}</p> : null}
          {needsPrivate(id) ? (
            <p className="text-xs text-ink-2">
              The gateway refuses this address until it is added to the allowed private destinations in Limits.
            </p>
          ) : null}
          <HarnessLine harnesses={harnesses?.[id]} />
          <RolesLine entry={e} />
          <RoleWarningLine id={id} helm={false} warnings={roleWarnings} onSetRoles={() => setEditing({ index: i })} />
          {bad ? (
            <ul className="list-disc pl-5 text-xs text-danger-ink">
              {Object.entries(errs).map(([f, m]) => (
                <li key={f}>
                  <code className="font-mono">{f}</code>: {m}
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      </ListRow>,
    );
  });
  return (
    <SettingField
      id={s.key}
      label="Endpoints"
      hint="Where agents' AI requests go. AI keys are never part of an endpoint."
      source={own.length > 0 ? "dashboard" : "default"}
      error={ctx.error}
    >
      <div id={settingControlId(s.key)} className="space-y-2">
        <ItemList aria-label="Endpoints">
          {rows.length === 0 ? <ListRow>No endpoint yet. Add one to let agents use a provider other than the built-in Anthropic API.</ListRow> : rows}
        </ItemList>
        <Button type="button" variant="outline" size="sm" disabled={providers.length === 0} onClick={() => setEditing({ index: null })}>
          Add endpoint…
        </Button>
        {providers.length === 0 ? <p className="text-xs text-muted">Turn on a provider above first.</p> : null}
      </div>
      {editing ? (
        <EndpointDialog
          key={editing.index ?? "new"}
          existing={editingEntry}
          providers={providers}
          helmIds={helmIds}
          otherIds={own.filter((_, j) => j !== editing.index).map(idOf)}
          errors={editingEntry ? errorsOf(idOf(editingEntry)) : {}}
          onSave={save}
          onClose={() => setEditing(null)}
        />
      ) : null}
      {viewing ? <HelmEndpointDialog entry={viewing} helmKey={s.helm_key} onClose={() => setViewing(null)} /> : null}
    </SettingField>
  );
}
