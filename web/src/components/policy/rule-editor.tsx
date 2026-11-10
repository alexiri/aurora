"use client";

import { useState, type FormEvent } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import {
  newTarget,
  normalizeRule,
  TARGET_TYPES,
  type Condition,
  type Identity,
  type RuleInput,
  type Target,
  type TargetType,
} from "@/lib/api/policy";

export interface Option {
  id: string;
  label: string;
}

export interface PickerData {
  agents: Option[];
  users: Option[];
  secrets: Option[];
  /** (P4-GRP-1) The org's groups; `undefined` while unknown (loading, or the caller may not list them). */
  groups?: Option[];
}

function IdentityRow({
  value,
  data,
  onChange,
  onRemove,
}: {
  value: Identity;
  data: PickerData;
  onChange: (v: Identity) => void;
  onRemove: () => void;
}) {
  const options = value.kind === "agent" ? data.agents : value.kind === "group" ? (data.groups ?? []) : data.users;
  // A group that no longer exists matches nobody (SEC-71): never a blank chip.
  const removedGroup = value.kind === "group" && value.id !== "" && data.groups !== undefined && !data.groups.some((g) => g.id === value.id);
  return (
    <li className="flex flex-wrap gap-2">
      <select
        aria-label="Identity kind"
        className={inputClass}
        value={value.kind}
        onChange={(e) => onChange({ kind: e.target.value as Identity["kind"], id: "" })}
      >
        <option value="agent">Agent</option>
        <option value="user">When a person is speaking</option>
        {data.groups !== undefined ? <option value="group">When a member of a group is speaking</option> : null}
      </select>
      {options.length > 0 ? (
        <select aria-label="Identity" className={inputClass} value={value.id} onChange={(e) => onChange({ ...value, id: e.target.value })}>
          <option value="">Select…</option>
          {options.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
      ) : (
        <input aria-label="Identity id" className={inputClass} placeholder="ID" value={value.id} onChange={(e) => onChange({ ...value, id: e.target.value })} />
      )}
      <Button type="button" variant="ghost" size="sm" onClick={onRemove} aria-label="Remove identity">
        Remove
      </Button>
      {value.kind === "user" ? (
        <p className="basis-full text-xs text-muted">
          Matches only turns this person is speaking in — never schedules, webhooks, BYO or boot traffic. For “anyone with access”, leave who empty.
        </p>
      ) : null}
      {value.kind === "group" ? (
        <p className="basis-full text-xs text-muted">
          Matches only turns spoken by a member of the group — never schedules, webhooks, BYO or boot traffic. For “anyone with access”, leave who empty.
        </p>
      ) : null}
      {removedGroup ? (
        <p role="status" className="basis-full">
          <Badge tone="danger">Group no longer exists — matches nobody</Badge>
        </p>
      ) : null}
    </li>
  );
}

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];

function TargetRow({
  value,
  data,
  onChange,
  onRemove,
}: {
  value: Target;
  data: PickerData;
  onChange: (v: Target) => void;
  onRemove: () => void;
}) {
  return (
    <li className="space-y-2 rounded-control border border-line p-2.5">
      <div className="flex flex-wrap gap-2">
        <select
          aria-label="Target type"
          className={inputClass}
          value={value.type}
          onChange={(e) => onChange(newTarget(e.target.value as TargetType, data.secrets[0]?.id))}
        >
          {TARGET_TYPES.map((t) => (
            <option key={t.type} value={t.type} disabled={t.disabled}>
              {t.label}
            </option>
          ))}
        </select>
        <Button type="button" variant="ghost" size="sm" onClick={onRemove} aria-label="Remove target">
          Remove
        </Button>
      </div>
      {value.type === "network" ? (
        <div className="grid grid-cols-2 gap-2">
          <input aria-label="Host pattern" className={inputClass} placeholder="api.example.com" value={value.host} onChange={(e) => onChange({ ...value, host: e.target.value })} />
          <input aria-label="Path pattern" className={inputClass} placeholder="/v1/*" value={value.path ?? ""} onChange={(e) => onChange({ ...value, path: e.target.value || null })} />
          <fieldset className="col-span-2 flex flex-wrap gap-3 text-sm">
            <legend className="sr-only">Methods</legend>
            {METHODS.map((m) => (
              <label key={m} className="flex items-center gap-1">
                <input
                  type="checkbox"
                  checked={value.methods?.includes(m) ?? false}
                  onChange={(e) => {
                    const cur = value.methods ?? [];
                    const next = e.target.checked ? [...cur, m] : cur.filter((x) => x !== m);
                    onChange({ ...value, methods: next.length ? next : null });
                  }}
                />
                {m}
              </label>
            ))}
            <span className="text-xs text-muted">(none = any method)</span>
          </fieldset>
        </div>
      ) : null}
      {value.type === "secret" ? (
        <select aria-label="Secret" className={inputClass} value={value.id} onChange={(e) => onChange({ ...value, id: e.target.value })}>
          <option value="">Select a secret…</option>
          {data.secrets.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
      ) : null}
      {value.type === "secrets" ? (
        <div className="grid grid-cols-2 gap-2">
          <select aria-label="Secret scope" className={inputClass} value={value.scope} onChange={(e) => onChange({ ...value, scope: e.target.value as "any" })}>
            <option value="any">Any scope</option>
            <option value="workspace">Workspace</option>
            <option value="organization">Organization</option>
          </select>
          <select aria-label="Secret kind" className={inputClass} value={value.kind ?? ""} onChange={(e) => onChange({ ...value, kind: (e.target.value || null) as "generic" | "llm" | null })}>
            <option value="">All kinds</option>
            <option value="generic">Generic</option>
            <option value="llm">LLM keys</option>
          </select>
        </div>
      ) : null}
    </li>
  );
}

function ConditionRow({
  value,
  onChange,
  onRemove,
}: {
  value: Condition;
  onChange: (v: Condition) => void;
  onRemove: () => void;
}) {
  return (
    <li className="flex gap-2">
      <select aria-label="Condition operator" className={inputClass} value={value.op} onChange={(e) => onChange({ ...value, op: e.target.value as Condition["op"] })}>
        <option value="contains">body contains</option>
        <option value="not_contains">body does not contain</option>
        <option value="equals">body equals</option>
      </select>
      <input aria-label="Condition value" className={inputClass} value={value.value} onChange={(e) => onChange({ ...value, value: e.target.value })} />
      <input aria-label="JSON pointer" className={inputClass} placeholder="/json/pointer (optional)" value={value.json_pointer ?? ""} onChange={(e) => onChange({ ...value, json_pointer: e.target.value || null })} />
      <Button type="button" variant="ghost" size="sm" onClick={onRemove} aria-label="Remove condition">
        Remove
      </Button>
    </li>
  );
}

function replaceAt<T>(a: T[], i: number, v: T): T[] {
  return a.map((x, j) => (j === i ? v : x));
}

export function RuleEditorDialog({
  initial,
  data,
  open,
  onClose,
  onSave,
}: {
  initial: RuleInput;
  data: PickerData;
  open: boolean;
  onClose: () => void;
  onSave: (r: RuleInput) => void;
}) {
  const [rule, setRule] = useState<RuleInput>(initial);
  const [error, setError] = useState<string | null>(null);
  const patch = (p: Partial<RuleInput>) => setRule((r) => ({ ...r, ...p }));
  const identities = rule.identities ?? [];
  const targets = rule.targets ?? [];
  const conditions = rule.conditions ?? [];
  const allow = rule.action === "allow";

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!rule.name.trim()) return setError("Give the rule a name.");
    onSave(normalizeRule({ ...rule, name: rule.name.trim() }));
  }

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent title={initial.logical_id ? "Edit rule" : "Add rule"} className="max-h-[90vh] w-[min(40rem,calc(100vw-2rem))] overflow-y-auto">
        <form onSubmit={submit} className="space-y-4" aria-label="Rule editor">
          <Field id="rule-name" label="Name" error={error ?? undefined}>
            <input id="rule-name" className={inputClass} value={rule.name} onChange={(e) => patch({ name: e.target.value })} />
          </Field>
          <Field id="rule-desc" label="Description">
            <input id="rule-desc" className={inputClass} value={rule.description ?? ""} onChange={(e) => patch({ description: e.target.value })} />
          </Field>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={rule.enabled ?? true} onChange={(e) => patch({ enabled: e.target.checked })} />
            Enabled
          </label>

          <section aria-label="Identities" className="space-y-2">
            <h3 className="text-sm font-medium">Who (empty = anyone with access)</h3>
            <ul className="space-y-2">
              {identities.map((v, i) => (
                <IdentityRow key={i} value={v} data={data} onChange={(n) => patch({ identities: replaceAt(identities, i, n) })} onRemove={() => patch({ identities: identities.filter((_, j) => j !== i) })} />
              ))}
            </ul>
            <Button type="button" variant="outline" size="sm" onClick={() => patch({ identities: [...identities, { kind: "agent", id: "" }] })}>
              Add identity
            </Button>
          </section>

          <section aria-label="Targets" className="space-y-2">
            <h3 className="text-sm font-medium">What</h3>
            <ul className="space-y-2">
              {targets.map((v, i) => (
                <TargetRow key={i} value={v} data={data} onChange={(n) => patch({ targets: replaceAt(targets, i, n) })} onRemove={() => patch({ targets: targets.filter((_, j) => j !== i) })} />
              ))}
            </ul>
            <Button type="button" variant="outline" size="sm" onClick={() => patch({ targets: [...targets, newTarget("network")] })}>
              Add target
            </Button>
          </section>

          <section aria-label="Conditions" className="space-y-2">
            <h3 className="text-sm font-medium">Conditions</h3>
            <ul className="space-y-2">
              {conditions.map((v, i) => (
                <ConditionRow key={i} value={v} onChange={(n) => patch({ conditions: replaceAt(conditions, i, n) })} onRemove={() => patch({ conditions: conditions.filter((_, j) => j !== i) })} />
              ))}
            </ul>
            <Button type="button" variant="outline" size="sm" onClick={() => patch({ conditions: [...conditions, { on: "body", op: "contains", value: "" }] })}>
              Add condition
            </Button>
          </section>

          <Field id="rule-action" label="Action">
            <select id="rule-action" className={inputClass} value={rule.action} onChange={(e) => patch({ action: e.target.value as RuleInput["action"] })}>
              <option value="allow">Allow</option>
              <option value="block">Block</option>
            </select>
          </Field>
          {allow ? (
            <fieldset className="space-y-2" aria-label="Modifiers">
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={rule.require_approval ?? false}
                  onChange={(e) =>
                    patch(e.target.checked ? { require_approval: true } : { require_approval: false, approval_timeout_secs: null })
                  }
                />
                Require approval
              </label>
              {rule.require_approval ? (
                <div className="flex flex-wrap items-center gap-2 pl-6 text-sm">
                  <label htmlFor="rule-approval-timeout">Approval timeout (seconds)</label>
                  <input
                    id="rule-approval-timeout"
                    type="number"
                    min={60}
                    inputMode="numeric"
                    placeholder="default"
                    className={inputClass + " w-28"}
                    value={rule.approval_timeout_secs ?? ""}
                    onChange={(e) => patch({ approval_timeout_secs: e.target.value === "" ? null : Number(e.target.value) })}
                  />
                  <span className="text-xs text-muted">
                    Empty = the instance default (5 min). At least 60 s, at most the instance maximum (1 h unless raised).
                  </span>
                </div>
              ) : null}
              <div className="flex items-center gap-2 text-sm">
                <label className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={!!rule.rate_limit}
                    onChange={(e) => patch({ rate_limit: e.target.checked ? { count: 10, per: "minute" } : null })}
                  />
                  Rate limit
                </label>
                {rule.rate_limit ? (
                  <>
                    <input
                      aria-label="Rate limit count"
                      type="number"
                      min={1}
                      className={inputClass + " w-24"}
                      value={rule.rate_limit.count}
                      onChange={(e) => patch({ rate_limit: { ...rule.rate_limit!, count: Number(e.target.value) } })}
                    />
                    <select aria-label="Rate limit period" className={inputClass + " w-32"} value={rule.rate_limit.per} onChange={(e) => patch({ rate_limit: { ...rule.rate_limit!, per: e.target.value as "minute" } })}>
                      <option value="minute">per minute</option>
                      <option value="hour">per hour</option>
                      <option value="day">per day</option>
                    </select>
                  </>
                ) : null}
              </div>
            </fieldset>
          ) : null}

          <div className="flex justify-end gap-2">
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit">Save rule</Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
