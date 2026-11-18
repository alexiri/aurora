"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions, FormSection } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { useHarnesses, usePatchAgent } from "@/lib/api/agents";
import { ApiError } from "@/lib/api/client";
import { useModelReadiness } from "@/lib/api/credential-grants";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useModels } from "@/lib/api/conversations";
import type { Agent } from "@/lib/api/queries";
import { errorTarget, ROLES } from "@/lib/model-roles";

import { AvatarField } from "./avatar-field";
import { EMPTY_ROLES, ModelRolesFields, type RoleDraft, type RoleFieldErrors } from "./model-roles-fields";

export function InstructionsTab({ agent, wsId }: { agent: Agent; wsId: string }) {
  const patch = usePatchAgent(wsId, agent.id);
  const [name, setName] = useState(agent.name);
  const [brief, setBrief] = useState(agent.brief);
  const [effort, setEffort] = useState(agent.effort ?? "");
  const [proactive, setProactive] = useState(agent.proactive_messaging);
  const models = useModels(agent.id, agent.kind === "hosted");
  const [model, setModel] = useState(
    agent.model_endpoint_id && agent.model_id ? `${agent.model_endpoint_id}\u0000${agent.model_id}` : "",
  );
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState(false);
  const fe = patch.error ? fieldErrors(patch.error) : {};

  // (phase 5, P5-MODELS-3, D-302, D-303, C-876) Roles belong to the default model's endpoint: the one chosen here, or
  // the one the next turn would use (readiness, owners only).
  const readiness = useModelReadiness(agent.id, agent.kind === "hosted");
  const defaultView = readiness.data?.default ?? null;
  const endpointId = model ? model.split("\u0000")[0]! : (defaultView?.endpoint_id ?? agent.model_endpoint_id ?? null);
  const storedRoles = agent.model_roles ?? null;
  const rolesFor = (ep: string | null): RoleDraft =>
    storedRoles && ep && storedRoles.endpoint_id === ep
      ? { large: storedRoles.large ?? "", medium: storedRoles.medium ?? "", small: storedRoles.small ?? "" }
      : EMPTY_ROLES;
  const [roles, setRoles] = useState<RoleDraft>(() => rolesFor(endpointId));
  const [subagent, setSubagent] = useState(agent.subagent_model);
  const roleErrors = roleFieldErrors(patch.error);

  // (phase 4, P4-HAR) The picker exists only for hosted agents and only when more than one
  // harness is enabled; the server stays authoritative (422 not enabled, 409 turn running).
  const harnesses = useHarnesses(agent.kind === "hosted");
  const options = harnesses.data?.items ?? [];
  const showPicker = agent.kind === "hosted" && options.length > 1;
  const currentHarness = agent.harness ?? harnesses.data?.default ?? "";
  const [harness, setHarness] = useState(agent.harness ?? "");
  const chosenHarness = harness || currentHarness;
  const harnessChanged = showPicker && chosenHarness !== currentHarness;
  const [confirmSwitch, setConfirmSwitch] = useState(false);
  const harnessInfo = options.find((h) => h.id === chosenHarness);
  // Only a harness that is known to map no roles hides the group; while the list loads the group stays.
  const roleless = agent.kind === "hosted" && harnessInfo !== undefined && harnessInfo.model_roles.length === 0;
  const endpointModels = (models.data?.items ?? []).filter((m) => m.endpoint_id === endpointId);

  /** `model_roles` of the PATCH when the roles changed: null clears, an object replaces (C-876). */
  function rolesPatch(): { model_roles: { endpoint_id: string; large?: string; medium?: string; small?: string } | null } | null {
    if (agent.kind !== "hosted" || roleless) return null;
    const set = ROLES.filter((r) => roles[r] !== "");
    const next = set.length === 0 || !endpointId ? null : { endpoint_id: endpointId, ...Object.fromEntries(set.map((r) => [r, roles[r]])) };
    const same =
      next === null
        ? storedRoles === null || (!storedRoles.large && !storedRoles.medium && !storedRoles.small)
        : storedRoles !== null &&
          storedRoles.endpoint_id === endpointId &&
          ROLES.every((r) => (storedRoles[r] ?? "") === roles[r]);
    return same ? null : { model_roles: next };
  }

  async function save(n: string) {
    try {
      await patch.mutateAsync({
        name: n,
        brief,
        proactive_messaging: proactive,
        ...(harnessChanged ? { harness: chosenHarness } : {}),
        // "Default" is `null` (the API clears the override); omitting it would keep the old value (WEBA-1).
        effort: effort || null,
        ...(model ? { model_endpoint_id: model.split("\u0000")[0], model_id: model.split("\u0000")[1] } : {}),
        ...(rolesPatch() ?? {}),
        ...(agent.kind === "hosted" && !roleless && subagent !== agent.subagent_model ? { subagent_model: subagent } : {}),
      });
      setSaved(true);
    } catch {
      // surfaced via patch.error
    }
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const n = name.trim();
    if (!n || n.length > 80) return setClientErr("Name must be 1 to 80 characters.");
    setClientErr("");
    if (harnessChanged) return setConfirmSwitch(true);
    await save(n);
  }

  return (
    <div className="space-y-6">
      <Section id="avatar" title="Avatar" card>
        <AvatarField agent={agent} wsId={wsId} />
      </Section>
      <Form onSubmit={(e) => void submit(e)} noValidate className="max-w-2xl">
      <FormSection title="Details" description="What this agent is called and how it behaves.">
      <Field id="a-name" label="Name" error={clientErr || fe.name}>
        <input id="a-name" className={inputClass} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="a-handle" label="Handle" hint="The handle cannot be changed.">
        <input id="a-handle" className={inputClass} value={agent.handle} readOnly />
      </Field>
      <Field id="a-brief" label="Brief" error={fe.brief}>
        <textarea id="a-brief" className={inputClass} rows={5} value={brief} onChange={(e) => setBrief(e.target.value)} />
      </Field>
      <Field
        id="a-harness"
        label="Harness"
        error={fe.harness}
        hint={showPicker ? "Changing it starts fresh sessions for every conversation of this agent." : undefined}
      >
        {showPicker ? (
          <select id="a-harness" className={inputClass} value={chosenHarness} onChange={(e) => setHarness(e.target.value)}>
            {options.map((h) => (
              <option key={h.id} value={h.id} disabled={!h.available && h.id !== currentHarness}>
                {h.display_name}
                {h.available ? "" : " (no runner offers this)"}
              </option>
            ))}
          </select>
        ) : (
          <input id="a-harness" className={inputClass} value={agent.harness ?? ""} readOnly disabled />
        )}
      </Field>
      <Field
        id="a-model"
        label="Default model"
        error={roleErrors.model}
        hint={modelHint(agent.kind, models.data?.items.length, models.isError)}
      >
        <select
          id="a-model"
          className={inputClass}
          value={model}
          disabled={agent.kind !== "hosted" || !models.data?.items.length}
          onChange={(e) => {
            setModel(e.target.value);
            setRoles(rolesFor(e.target.value ? e.target.value.split("\u0000")[0]! : (defaultView?.endpoint_id ?? null)));
          }}
        >
          <option value="">Default</option>
          {model && !models.data?.items.some((m) => `${m.endpoint_id}\u0000${m.model_id}` === model) ? (
            <option value={model}>{model.replace("\u0000", " / ")}</option>
          ) : null}
          {(models.data?.items ?? []).map((m) => (
            <option key={`${m.endpoint_id}/${m.model_id}`} value={`${m.endpoint_id}\u0000${m.model_id}`}>
              {m.display_name ?? m.model_id} ({m.provider})
            </option>
          ))}
        </select>
      </Field>
      {agent.kind === "hosted" ? (
        <ModelRolesFields
          harnessId={chosenHarness || undefined}
          harnessName={harnessInfo?.display_name ?? "this harness"}
          roleless={roleless}
          mapped={harnessInfo?.model_roles ?? null}
          models={endpointModels}
          roles={roles}
          onRoles={setRoles}
          subagent={subagent}
          onSubagent={setSubagent}
          readiness={defaultView}
          errors={roleErrors}
          disabled={!endpointId}
        />
      ) : null}
      <Field id="a-effort"label="Effort" error={fe.effort}>
        <select id="a-effort" className={inputClass} value={effort} onChange={(e) => setEffort(e.target.value)}>
          <option value="">Default</option>
          {["low", "medium", "high", "max"].map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
      </Field>
      <Field id="a-proactive" label="Proactive messaging" error={fe.proactive_messaging}>
        <select id="a-proactive" className={inputClass} value={proactive} onChange={(e) => setProactive(e.target.value)}>
          <option value="ask">Ask first</option>
          <option value="allow">Allow</option>
          <option value="off">Off</option>
        </select>
      </Field>
      {patch.error && Object.keys(fe).length === 0 && !hasRoleError(roleErrors) ? (
        <Notice tone="danger" role="alert">
          {errorMessage(patch.error)}
        </Notice>
      ) : null}
      </FormSection>
      <FormActions>
        <Button type="submit" size="lg" disabled={patch.isPending}>
          {patch.isPending ? "Saving…" : "Save"}
        </Button>
        {saved ? (
          <span role="status" className="text-sm text-accent-ink">
            Saved
          </span>
        ) : null}
      </FormActions>
      </Form>
      <ConfirmDialog
        open={confirmSwitch}
        onOpenChange={setConfirmSwitch}
        title="Change the harness?"
        description="Starts fresh sessions for every conversation of this agent; the transcript is kept."
        confirmLabel="Change harness"
        busy={patch.isPending}
        onConfirm={() => {
          setConfirmSwitch(false);
          void save(name.trim());
        }}
      />
    </div>
  );
}

function modelHint(kind: string, count: number | undefined, isError: boolean): string {
  if (kind !== "hosted") return "Models apply to hosted agents.";
  if (isError) return "Could not load models.";
  if (count === 0) return "Available when an LLM key is granted.";
  return "The model the agent starts with. It may use other allowed models on the same endpoint, for example for subagents.";
}

const MESSAGE_MISMATCH =
  "These roles belong to another endpoint than the default model. Choose the roles again for this endpoint.";

/** (C-876) Map `model_role_endpoint_mismatch`, `llm_model_not_allowed` and `model_id_invalid` onto the field that caused them. */
function hasRoleError(e: RoleFieldErrors & { model?: string }): boolean {
  return Boolean(e.group || e.subagent || e.model || Object.keys(e.roles).length);
}

function roleFieldErrors(err: unknown): RoleFieldErrors & { model?: string } {
  const out: RoleFieldErrors & { model?: string } = { roles: {} };
  if (!(err instanceof ApiError)) return out;
  const d = (typeof err.details === "object" && err.details !== null ? err.details : {}) as Record<string, unknown>;
  if (err.code === "model_role_endpoint_mismatch") {
    out.group = MESSAGE_MISMATCH;
  } else if (err.code === "llm_model_not_allowed" || err.code === "model_id_invalid") {
    const target = errorTarget(d.field);
    const model = typeof d.model === "string" ? `${d.model} is` : "That model is";
    const text =
      err.code === "llm_model_not_allowed" ? `${model} not allowed on this endpoint.` : "That is not a valid model id.";
    if (target.role) out.roles[target.role] = text;
    else if (target.subagent) out.subagent = text;
    else out.model = text;
  }
  return out;
}
