"use client";

import { useState } from "react";

import { DynamicForm } from "@/components/apps/dynamic-form";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { visibleFields } from "@/lib/api/apps";
import { ApiError } from "@/lib/api/client";
import {
  setupFields,
  useConnectorTypes,
  useCreateManagedConnector,
  type ConnectorType,
  type CreateConnectorRequest,
} from "@/lib/api/connectors";

import { ManifestSummary } from "./manifest-views";
import { ConnectorError, ConnectorIcon, UnsignedBadge, settingErrors } from "./shared";
import { requiredErrors, typedValue } from "./setup-values";

type Step = "choose" | "review" | "settings";

/** First line of a setup guide as plain text, for the type card. */
export function shortDescription(md: string | null | undefined): string {
  const line = (md ?? "").split("\n").map((l) => l.trim()).find((l) => l && !l.startsWith("#")) ?? "";
  const text = line.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1").replace(/[*_`>]/g, "").trim();
  return text.length > 160 ? `${text.slice(0, 157)}…` : text;
}

function TypeList({ items, onPick }: { items: ConnectorType[]; onPick: (t: ConnectorType) => void }) {
  return (
    <ItemList aria-label="Connector types">
      {items.map((t) => {
        const desc = shortDescription(t.setup_guide_md);
        return (
          <ListRow
            key={t.type_id}
            actions={
              t.added ? (
                <Badge tone="accent">Added</Badge>
              ) : (
                <Button variant="outline" size="sm" aria-label={`Add ${t.name}`} onClick={() => onPick(t)}>
                  Add
                </Button>
              )
            }
          >
            <ConnectorIcon id={t.type_id} name={t.name} iconB64={t.icon_png_b64} hasIcon={false} />
            <div className="min-w-0 flex-1 basis-48">
              <p className="flex flex-wrap items-center gap-x-2 text-sm font-semibold text-ink">
                {t.name}
                <span className="text-xs font-normal text-muted">
                  v{t.version} · {t.publisher}
                </span>
                {t.signed === false ? <UnsignedBadge /> : null}
              </p>
              {desc ? <p className="text-sm text-ink-2">{desc}</p> : null}
            </div>
          </ListRow>
        );
      })}
    </ItemList>
  );
}

function NoHost() {
  return (
    <EmptyState title="The connector host is not running">
      No connector host has reported in the last 90 seconds, so there are no connector types to add. Start the host and
      reload; see <code className="font-mono">docs/operator/plugins-and-connectors.md</code>.
    </EmptyState>
  );
}

function Review({ type, onBack, onNext, stale }: { type: ConnectorType; onBack: () => void; onNext: () => void; stale: boolean }) {
  return (
    <Section id="add-review" title={`Review ${type.name}`} description="What this connector can do, before you add it." card>
      <div className="space-y-4">
        {stale ? (
          <Notice tone="pending" role="status">
            The connector type changed while you were adding it. The latest version is shown; review it again.
          </Notice>
        ) : null}
        <div className="flex flex-wrap items-center gap-3">
          <ConnectorIcon id={type.type_id} name={type.name} iconB64={type.icon_png_b64} hasIcon={false} />
          <div className="min-w-0">
            <p className="text-sm font-semibold text-ink">{type.name}</p>
            <p className="text-xs text-muted">
              v{type.version} · {type.publisher}
            </p>
          </div>
          {type.signed === false ? <UnsignedBadge /> : <Badge tone="accent">Signature verified</Badge>}
        </div>
        <ManifestSummary manifest={type.manifest} />
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" onClick={onBack}>
            Back
          </Button>
          <Button onClick={onNext}>Continue</Button>
        </div>
      </div>
    </Section>
  );
}

function SettingsStep({
  type,
  values,
  setValues,
  displayName,
  setDisplayName,
  onBack,
  onAdded,
  onStale,
}: {
  type: ConnectorType;
  values: Record<string, string>;
  setValues: (v: Record<string, string>) => void;
  displayName: string;
  setDisplayName: (v: string) => void;
  onBack: () => void;
  onAdded: (id: string) => void;
  onStale: () => void;
}) {
  const create = useCreateManagedConnector();
  const fields = setupFields(type.instance_setup);
  const [errors, setErrors] = useState<Record<string, string>>({});

  function submit() {
    const shown = visibleFields(fields, values) as typeof fields;
    const missing = requiredErrors(shown, values, (f) => (values[f.name] ?? "") !== "");
    setErrors(missing);
    if (Object.keys(missing).length) return;
    const settings: Record<string, unknown> = {};
    const secrets: Record<string, string> = {};
    for (const f of shown) {
      const v = values[f.name] ?? "";
      if (v === "" || f.type === "generated_secret") continue;
      if (f.type === "secret") secrets[f.name] = v;
      else settings[f.name] = typedValue(f, v);
    }
    const body: CreateConnectorRequest = {
      id: type.type_id,
      approve_manifest_digest: type.manifest_digest,
      // The generated type of an open JSON object is `Record<string, never>`.
      settings: settings as Record<string, never>,
      secrets,
    };
    if (displayName.trim()) body.display_name = displayName.trim();
    create.mutate(body, {
      onSuccess: (r) => onAdded(r.connector.id),
      onError: (e) => {
        if (e instanceof ApiError && e.code === "stale_manifest") return onStale();
        setErrors(
          settingErrors(
            e,
            fields.map((f) => f.name),
          ),
        );
      },
    });
  }

  const apiFieldErrors = create.error && Object.keys(errors).length > 0;
  return (
    <Section id="add-settings" title={`Settings for ${type.name}`} description="Secrets are saved once and never shown again." card>
      <form
        noValidate
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <Field id="conn-display-name" label="Display name (optional)" hint={`Shown to administrators; defaults to ${type.name}.`}>
          <input
            id="conn-display-name"
            className={inputClass}
            maxLength={80}
            autoComplete="off"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
          />
        </Field>
        {fields.length === 0 ? <p className="text-sm text-ink-2">This connector has no instance settings.</p> : null}
        <DynamicForm
          fields={fields}
          values={values}
          errors={errors}
          onChange={(name, v) => setValues({ ...values, [name]: v })}
        />
        {create.error && !apiFieldErrors ? <ConnectorError error={create.error} /> : null}
        {apiFieldErrors ? (
          <Notice tone="danger" role="alert">
            Some settings were not accepted. Fix the marked fields and add again.
          </Notice>
        ) : null}
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" onClick={onBack}>
            Back
          </Button>
          <Button type="submit" disabled={create.isPending}>
            Add connector
          </Button>
        </div>
      </form>
    </Section>
  );
}

/** Add a managed connector: choose a type, review it, fill in its settings, add (C-450, C-451). */
export function AddConnectorFlow({
  onSelfHosted,
  onAdded,
}: {
  onSelfHosted: () => void;
  onAdded: (id: string) => void;
}) {
  const q = useConnectorTypes();
  const [step, setStep] = useState<Step>("choose");
  const [typeId, setTypeId] = useState<string | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [displayName, setDisplayName] = useState("");
  const [stale, setStale] = useState(false);

  const type = q.data?.items.find((t) => t.type_id === typeId && !t.added);
  if (step !== "choose" && type) {
    if (step === "review") {
      return <Review type={type} stale={stale} onBack={() => setStep("choose")} onNext={() => setStep("settings")} />;
    }
    return (
      <SettingsStep
        type={type}
        values={values}
        setValues={setValues}
        displayName={displayName}
        setDisplayName={setDisplayName}
        onBack={() => setStep("review")}
        onAdded={onAdded}
        onStale={() => {
          setStale(true);
          setStep("review");
        }}
      />
    );
  }

  return (
    <Section
      id="add-choose"
      title="Choose a connector type"
      description="Types offered by the running connector host."
      actions={
        <Button variant="ghost" size="sm" onClick={onSelfHosted}>
          Self-hosted connector (advanced)
        </Button>
      }
    >
      {q.isPending ? (
        <Loading />
      ) : q.isError ? (
        <ErrorState error={q.error} onRetry={() => q.refetch()} />
      ) : !q.data.host_running ? (
        <NoHost />
      ) : (
        <TypeList
          items={q.data.items}
          onPick={(t) => {
            setTypeId(t.type_id);
            setValues({});
            setDisplayName("");
            setStale(false);
            setStep("review");
          }}
        />
      )}
    </Section>
  );
}
