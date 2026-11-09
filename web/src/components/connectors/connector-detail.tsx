"use client";

import { useState } from "react";

import { DynamicForm, FieldControl } from "@/components/apps/dynamic-form";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { DangerZone, ItemList, ListRow, Section } from "@/components/ui/section";
import {
  setupFields,
  useConnectorTypes,
  useDeleteConnector,
  usePatchConnector,
  type ConnectorAdmin,
  type PatchConnectorRequest,
  type SetupField,
} from "@/lib/api/connectors";

import { ApprovalPanel } from "./approval-panel";
import { ConnectorError, ConnectorIcon, ConnectorStatusChip, ManagedBadge, count, fmtWhen, settingErrors } from "./shared";
import { initialValues, isSecretField, requiredErrors, settingText, typedValue } from "./setup-values";

type Staged = "replace" | "clear";

function HostStatus({ c }: { c: ConnectorAdmin }) {
  return (
    <Section id="detail-hosts" title="Connector hosts" description="Hosts that reported this connector recently.">
      {c.host_status.length === 0 ? (
        <p className="text-sm text-ink-2">No connector host has reported this connector yet.</p>
      ) : (
        <ItemList aria-label="Connector hosts">
          {c.host_status.map((h) => (
            <ListRow key={h.host_instance} actions={<span className="text-xs text-muted">{fmtWhen(h.at)}</span>}>
              <Badge tone={h.state === "running" ? "accent" : "danger"}>{h.state === "running" ? "Running" : "Error"}</Badge>
              <code className="font-mono text-xs text-muted">{h.host_instance.slice(0, 8)}</code>
              {h.message ? <span className="min-w-0 break-words text-sm text-ink-2">{h.message}</span> : null}
            </ListRow>
          ))}
        </ItemList>
      )}
    </Section>
  );
}

/** A stored secret: never its value, only whether and when it was set. */
function SecretRow({
  f,
  c,
  staged,
  typed,
  error,
  onStage,
  onType,
  onRegenerate,
}: {
  f: SetupField;
  c: ConnectorAdmin;
  staged?: Staged;
  typed: string;
  error?: string;
  onStage: (s: Staged | undefined) => void;
  onType: (v: string) => void;
  onRegenerate: () => void;
}) {
  const info = c.secrets[f.name];
  const generated = f.type === "generated_secret";
  const labelId = `secret-${f.name}-label`;
  if (staged === "replace" && !generated) {
    return (
      <div className="space-y-2">
        <FieldControl f={f} value={typed} error={error} onChange={onType} />
        <Button type="button" variant="ghost" size="sm" onClick={() => onStage(undefined)}>
          Cancel replace
        </Button>
      </div>
    );
  }
  const state = staged === "clear" ? "Will be cleared when you save" : info?.set ? `${generated ? "Generated automatically" : "Set"} · updated ${fmtWhen(info.updated_at)}` : generated ? "Generated automatically" : "Not set";
  return (
    <div role="group" aria-labelledby={labelId} data-testid={`secret-${f.name}`} className="space-y-1.5">
      <p id={labelId} className="text-sm font-medium text-ink">
        {f.label + (f.required || generated ? "" : " (optional)")}
      </p>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <p className="min-w-0 rounded-control border border-line bg-subtle px-3 py-2 text-sm text-ink-2">{state}</p>
        {generated ? (
          <Button type="button" variant="outline" size="sm" aria-label={`Regenerate ${f.label}`} onClick={onRegenerate}>
            Regenerate
          </Button>
        ) : staged === "clear" ? (
          <Button type="button" variant="outline" size="sm" onClick={() => onStage(undefined)}>
            Undo
          </Button>
        ) : (
          <>
            <Button type="button" variant="outline" size="sm" aria-label={`${info?.set ? "Replace" : "Set"} ${f.label}`} onClick={() => onStage("replace")}>
              {info?.set ? "Replace" : "Set"}
            </Button>
            {!f.required && info?.set ? (
              <Button type="button" variant="ghost" size="sm" aria-label={`Clear ${f.label}`} onClick={() => onStage("clear")}>
                Clear
              </Button>
            ) : null}
          </>
        )}
      </div>
      {error ? (
        <p role="alert" className="text-xs text-danger-ink">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function SettingsForm({ c, fields }: { c: ConnectorAdmin; fields: SetupField[] }) {
  const patch = usePatchConnector();
  const [values, setValues] = useState(() => initialValues(fields, c.settings));
  const [name, setName] = useState(c.display_name);
  const [staged, setStaged] = useState<Record<string, Staged | undefined>>({});
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [regen, setRegen] = useState<SetupField | null>(null);
  const [saved, setSaved] = useState(false);

  const names = fields.map((f) => f.name);
  const reset = (next: ConnectorAdmin) => {
    setValues(initialValues(fields, next.settings));
    setName(next.display_name);
    setStaged({});
    setTyped({});
    setErrors({});
  };

  function send(body: PatchConnectorRequest, done?: () => void) {
    setSaved(false);
    patch.mutate(
      { id: c.id, body },
      {
        onSuccess: (next) => {
          reset(next);
          setSaved(true);
          done?.();
        },
        onError: (e) => setErrors(settingErrors(e, names)),
      },
    );
  }

  function save() {
    const have = (f: SetupField) => (staged[f.name] === "replace" ? (typed[f.name] ?? "") !== "" : !!c.secrets[f.name]?.set && staged[f.name] !== "clear");
    const missing = requiredErrors(fields, values, have);
    // A staged replace with no text is not a change; only report what the user can fix.
    setErrors(missing);
    if (Object.keys(missing).length) return;
    const body: PatchConnectorRequest = {};
    if (name.trim() && name.trim() !== c.display_name) body.display_name = name.trim();
    const settings: Record<string, unknown> = {};
    const stored = c.settings as Record<string, unknown> | null | undefined;
    for (const f of fields) {
      if (isSecretField(f)) continue;
      const now = values[f.name] ?? "";
      if (now === settingText(stored?.[f.name])) continue;
      settings[f.name] = now === "" ? null : typedValue(f, now);
    }
    if (Object.keys(settings).length) body.settings = settings as Record<string, never>;
    const secrets: Record<string, string | null> = {};
    for (const f of fields) {
      if (f.type !== "secret") continue;
      if (staged[f.name] === "clear") secrets[f.name] = null;
      else if (staged[f.name] === "replace" && (typed[f.name] ?? "") !== "") secrets[f.name] = typed[f.name];
    }
    if (Object.keys(secrets).length) body.secrets = secrets;
    if (Object.keys(body).length === 0) {
      setSaved(true);
      return;
    }
    send(body);
  }

  const override = (f: SetupField, error?: string) =>
    isSecretField(f) ? (
      <SecretRow
        f={f}
        c={c}
        staged={staged[f.name]}
        typed={typed[f.name] ?? ""}
        error={error}
        onStage={(s) => setStaged({ ...staged, [f.name]: s })}
        onType={(v) => setTyped({ ...typed, [f.name]: v })}
        onRegenerate={() => setRegen(f)}
      />
    ) : undefined;

  return (
    <Section id="detail-settings" title="Settings" description="Secrets are never shown. Replace one to change it." card>
      <form
        noValidate
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <Field id="conn-detail-name" label="Display name">
          <input
            id="conn-detail-name"
            className={inputClass}
            maxLength={80}
            autoComplete="off"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        {fields.length === 0 ? <p className="text-sm text-ink-2">This connector has no instance settings.</p> : null}
        <DynamicForm
          fields={fields}
          values={values}
          errors={errors}
          onChange={(n, v) => setValues({ ...values, [n]: v })}
          override={(f, e) => override(f as SetupField, e)}
        />
        <ConnectorError error={patch.error} />
        {saved && !patch.error ? (
          <Notice tone="accent" role="status">
            Saved.
          </Notice>
        ) : null}
        <Button type="submit" disabled={patch.isPending}>
          Save settings
        </Button>
      </form>
      <ConfirmDialog
        open={regen !== null}
        onOpenChange={(o) => !o && setRegen(null)}
        title={`Regenerate ${regen?.label ?? ""}?`}
        description="A new value is generated and the connector restarts with it. Anything still using the old value stops working."
        confirmLabel="Regenerate"
        busy={patch.isPending}
        onConfirm={() => {
          if (regen) send({ regenerate: [regen.name] }, () => setRegen(null));
        }}
      />
    </Section>
  );
}

/** A managed connector: settings, enable/disable, host status, approval, delete. */
export function ConnectorDetail({ c, onBack }: { c: ConnectorAdmin; onBack: () => void }) {
  const types = useConnectorTypes();
  const toggle = usePatchConnector();
  const del = useDeleteConnector();
  const [confirmDel, setConfirmDel] = useState(false);
  const type = types.data?.items.find((t) => t.type_id === c.type_id);
  const fields = setupFields((type?.instance_setup ?? (c.manifest as { instance_setup?: unknown } | null)?.instance_setup) ?? null);
  const disabled = c.status === "disabled";
  const inUse = c.integration_count > 0;

  return (
    <div className="space-y-8">
      <PageHeader
        title={
          <span className="flex flex-wrap items-center gap-3">
            <ConnectorIcon id={c.id} name={c.display_name} iconB64={type?.icon_png_b64} />
            {c.display_name}
          </span>
        }
        description={
          <span className="flex flex-wrap items-center gap-2">
            <ManagedBadge managed />
            <ConnectorStatusChip c={c} />
            <span>
              {count(c.integration_count, "integration")}{c.version ? ` · v${c.version}` : ""}
            </span>
          </span>
        }
        actions={
          <>
            <Button variant="outline" onClick={onBack}>
              All connectors
            </Button>
            <Button
              variant={disabled ? "primary" : "outline"}
              disabled={toggle.isPending}
              onClick={() => toggle.mutate({ id: c.id, body: { enabled: disabled } })}
            >
              {disabled ? "Enable" : "Disable"}
            </Button>
          </>
        }
      />
      <ConnectorError error={toggle.error} />
      {c.status === "pending_approval" ? <ApprovalPanel c={c} /> : null}
      {/* Keyed by what the form was built from, so a reloaded type or saved settings reset it. */}
      <SettingsForm key={`${c.id}:${fields.map((f) => f.name).join(",")}`} c={c} fields={fields} />
      <HostStatus c={c} />
      <DangerZone
        id="detail-delete"
        title="Delete connector"
        description={
          inUse
            ? `${count(c.integration_count, "integration")} still ${c.integration_count === 1 ? "uses" : "use"} this connector. Remove ${c.integration_count === 1 ? "it" : "them"} first.`
            : "Removes the connector and its stored settings and secrets."
        }
      >
        <Button variant="danger" disabled={inUse} onClick={() => setConfirmDel(true)}>
          Delete
        </Button>
        <ConnectorError error={del.error} />
      </DangerZone>
      <ConfirmDialog
        open={confirmDel}
        onOpenChange={setConfirmDel}
        title={`Delete ${c.display_name}?`}
        description="Its settings and secrets are discarded. You can add it again later."
        confirmLabel="Delete connector"
        danger
        busy={del.isPending}
        onConfirm={() => del.mutate(c.id, { onSuccess: onBack, onSettled: () => setConfirmDel(false) })}
      />
    </div>
  );
}
