"use client";

import Link from "next/link";
import { useEffect, useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { SwitchField } from "@/components/ui/switch";
import { useHarnesses, usePatchAgent } from "@/lib/api/agents";
import { ApiError } from "@/lib/api/client";
import { useModelReadiness } from "@/lib/api/credential-grants";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useModels } from "@/lib/api/conversations";
import { useMe, type Agent } from "@/lib/api/queries";
import { errorTarget, ROLES } from "@/lib/model-roles";
import { adminRoutes } from "@/lib/routes";

import { AvatarField } from "./avatar-field";
import { automaticModelText, READINESS_WARNING_COPY } from "./model-readiness";
import { EMPTY_ROLES, ModelRolesFields, type RoleDraft, type RoleFieldErrors } from "./model-roles-fields";

/**
 * Behaviour › Instructions (P5-POLISH-6, C3, D-385): one card. The avatar (click it to change it) beside the Name, the
 * Instructions (the agent's brief, same data), the Model, "<Agent> may message you first" (a switch over the stored
 * proactive messaging: on is the stored Ask first or Allow, off is Off; switching on from Off stores Ask first), and a
 * collapsed **Advanced** with the harness, the model roles, Subagents use and Effort. No role warnings here (the
 * endpoint shows them). Save appears at the bottom of the viewport only when something changed.
 */
export function InstructionsTab({ agent, wsId }: { agent: Agent; wsId: string }) {
  const patch = usePatchAgent(wsId, agent.id);
  const isAdmin = useMe().data?.user.is_instance_admin === true;
  const [name, setName] = useState(agent.name);
  const [brief, setBrief] = useState(agent.brief);
  const [effort, setEffort] = useState(agent.effort ?? "");
  const [proactive, setProactive] = useState(agent.proactive_messaging);
  const models = useModels(agent.id, agent.kind === "hosted");
  const initialModel = agent.model_endpoint_id && agent.model_id ? `${agent.model_endpoint_id}\u0000${agent.model_id}` : "";
  const [model, setModel] = useState(initialModel);
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState(false);
  const fe = patch.error ? fieldErrors(patch.error) : {};

  // (FIX-170) The chat's "Choose a model" link ends in `#a-model`: scroll to the Model field and focus it
  // once it can take focus (it is disabled until the model list has loaded).
  const modelReady = agent.kind === "hosted" && !!models.data?.items.length;
  useEffect(() => {
    if (!modelReady || window.location.hash !== "#a-model") return;
    const el = document.getElementById("a-model");
    el?.scrollIntoView?.({ block: "center" });
    el?.focus();
  }, [modelReady]);

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

  const dirty =
    name !== agent.name ||
    brief !== agent.brief ||
    effort !== (agent.effort ?? "") ||
    proactive !== agent.proactive_messaging ||
    model !== initialModel ||
    harnessChanged ||
    rolesPatch() !== null ||
    (agent.kind === "hosted" && !roleless && subagent !== agent.subagent_model);

  function discard() {
    setName(agent.name);
    setBrief(agent.brief);
    setEffort(agent.effort ?? "");
    setProactive(agent.proactive_messaging);
    setModel(initialModel);
    setHarness(agent.harness ?? "");
    setRoles(rolesFor(agent.model_endpoint_id && agent.model_id ? agent.model_endpoint_id : (defaultView?.endpoint_id ?? null)));
    setSubagent(agent.subagent_model);
    setClientErr("");
    setSaved(false);
    patch.reset();
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

  const defaultModelLabel = defaultView ? `Workspace default · ${defaultView.model_id}` : "Workspace default";

  return (
    <div className="space-y-6">
      <form onSubmit={(e) => void submit(e)} noValidate className="max-w-180 space-y-5 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <div className="flex items-start gap-4">
          <AvatarField agent={agent} wsId={wsId} compact />
          <div className="min-w-0 flex-1">
            <Field id="a-name" label="Name" error={clientErr || fe.name}>
              <input id="a-name" className={inputClass} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
            </Field>
          </div>
        </div>
        <Field
          id="a-brief"
          label="Instructions"
          error={fe.brief}
          hint={`What ${agent.name} should do and how. It reads these before every conversation.`}>
          <textarea id="a-brief" className={inputClass} rows={6} value={brief} onChange={(e) => setBrief(e.target.value)} />
        </Field>
        <Field id="a-model" label="Model" error={roleErrors.model} hint={modelHint(agent.kind, models.data?.items.length, models.isError)}>
          <select
            id="a-model"
            className={inputClass}
            value={model}
            disabled={agent.kind !== "hosted" || !models.data?.items.length}
            onChange={(e) => {
              setModel(e.target.value);
              setRoles(rolesFor(e.target.value ? e.target.value.split("\u0000")[0]! : (defaultView?.endpoint_id ?? null)));
            }}>
            <option value="">{defaultModelLabel}</option>
            {model && !models.data?.items.some((m) => `${m.endpoint_id}\u0000${m.model_id}` === model) ? (
              <option value={model}>{model.replace("\u0000", " / ")}</option>
            ) : null}
            {(models.data?.items ?? []).map((m) => (
              <option key={`${m.endpoint_id}/${m.model_id}`} value={`${m.endpoint_id}\u0000${m.model_id}`}>
                {m.display_name ?? m.model_id} ({m.provider})
              </option>
            ))}
          </select>
          {/* (FIX-170, C-1026) What the next turn uses while no model is chosen, and the warnings of the default selection. */}
          {!model && defaultView ? (
            <p className="text-xs text-ink-2" data-testid="model-automatic">
              {automaticModelText(defaultView.source, defaultView.model_id)}
            </p>
          ) : null}
          {(readiness.data?.warnings ?? []).map((w) => (
            <p key={w} className="text-xs text-pending-ink" data-testid="model-warning">
              {READINESS_WARNING_COPY[w] ?? w}
            </p>
          ))}
        </Field>
        <SwitchField
          id="a-proactive"
          label={`${agent.name} may message you first`}
          hint="For example when a schedule finds something."
          checked={proactive !== "off"}
          onChange={(on) => setProactive(on ? (agent.proactive_messaging !== "off" ? agent.proactive_messaging : "ask") : "off")}
        />
        {fe.proactive_messaging ? (
          <p role="alert" className="text-xs text-danger-ink">
            {fe.proactive_messaging}
          </p>
        ) : null}
        <details className="rounded-control border border-line px-3 py-2">
          <summary className="cursor-pointer text-sm font-medium text-ink">Advanced: model roles, effort, agent program</summary>
          <div className="mt-3 space-y-5">
            <Field
              id="a-harness"
              label="Harness"
              error={fe.harness}
              hint={
                showPicker ? (
                  "Changing it starts fresh sessions for every conversation of this agent."
                ) : (
                  <>
                    Your administrator chooses which programs agents can run.
                    {isAdmin ? (
                      <>
                        {" "}
                        <Link href={adminRoutes.agents("defaults")} className="underline">
                          Administration › Agents › Defaults
                        </Link>
                      </>
                    ) : null}
                  </>
                )
              }>
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
                // Read-only text, not an input-looking box (D-374): the display name, or the id while the list loads.
                <p id="a-harness" className="text-sm text-ink">
                  {options.find((h) => h.id === (agent.harness ?? harnesses.data?.default))?.display_name ??
                    agent.harness ??
                    harnesses.data?.default ??
                    "—"}
                </p>
              )}
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
            <Field id="a-effort" label="Effort" error={fe.effort}>
              <select id="a-effort" className={inputClass} value={effort} onChange={(e) => setEffort(e.target.value)}>
                <option value="">Default</option>
                {["low", "medium", "high", "max"].map((o) => (
                  <option key={o} value={o}>
                    {o}
                  </option>
                ))}
              </select>
            </Field>
          </div>
        </details>
        {patch.error && Object.keys(fe).length === 0 && !hasRoleError(roleErrors) ? (
          <Notice tone="danger" role="alert">
            {errorMessage(patch.error)}
          </Notice>
        ) : null}
        {saved ? (
          <p role="status" className="text-sm text-accent-ink">
            Saved
          </p>
        ) : null}
        {/* (P5-POLISH-6) The save bar: only after a change, at the bottom of the viewport. There is no Save at the end of the page. */}
        {dirty ? (
          <div
            role="region"
            aria-label="Unsaved changes"
            className="fixed inset-x-4 bottom-4 z-30 mx-auto flex max-w-xl items-center justify-between gap-3 rounded-card border border-line bg-surface px-4 py-3 shadow-pop">
            <span className="text-sm font-medium text-ink">Unsaved changes</span>
            <span className="flex items-center gap-2">
              <Button type="button" variant="outline" size="sm" onClick={discard} disabled={patch.isPending}>
                Discard
              </Button>
              <Button type="submit" size="sm" disabled={patch.isPending}>
                {patch.isPending ? "Saving…" : "Save"}
              </Button>
            </span>
          </div>
        ) : null}
      </form>
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
