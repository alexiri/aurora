"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { usePatchAgent } from "@/lib/api/agents";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useModels } from "@/lib/api/conversations";
import type { Agent } from "@/lib/api/queries";

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

  async function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const n = name.trim();
    if (!n || n.length > 80) return setClientErr("Name must be 1 to 80 characters.");
    setClientErr("");
    try {
      await patch.mutateAsync({
        name: n,
        brief,
        proactive_messaging: proactive,
        ...(effort ? { effort } : {}),
        ...(model ? { model_endpoint_id: model.split("\u0000")[0], model_id: model.split("\u0000")[1] } : {}),
      });
      setSaved(true);
    } catch {
      // surfaced via patch.error
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="max-w-xl space-y-4">
      <Field id="a-name" label="Name" error={clientErr || fe.name}>
        <input id="a-name" className={inputClass} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="a-handle" label="Handle" hint="The handle cannot be changed.">
        <input id="a-handle" className={inputClass} value={agent.handle} readOnly />
      </Field>
      <Field id="a-brief" label="Brief" error={fe.brief}>
        <textarea id="a-brief" className={inputClass} rows={5} value={brief} onChange={(e) => setBrief(e.target.value)} />
      </Field>
      <Field id="a-harness" label="Harness">
        <input id="a-harness" className={inputClass} value={agent.harness ?? ""} readOnly disabled />
      </Field>
      <Field id="a-model" label="Model" hint={modelHint(agent.kind, models.data?.items.length, models.isError)}>
        <select
          id="a-model"
          className={inputClass}
          value={model}
          disabled={agent.kind !== "hosted" || !models.data?.items.length}
          onChange={(e) => setModel(e.target.value)}
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
      <Field id="a-proactive" label="Proactive messaging" error={fe.proactive_messaging}>
        <select id="a-proactive" className={inputClass} value={proactive} onChange={(e) => setProactive(e.target.value)}>
          <option value="ask">Ask first</option>
          <option value="allow">Allow</option>
          <option value="off">Off</option>
        </select>
      </Field>
      {patch.error && Object.keys(fe).length === 0 ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(patch.error)}
        </p>
      ) : null}
      <div className="flex items-center gap-3">
        <Button type="submit" disabled={patch.isPending}>
          {patch.isPending ? "Saving…" : "Save"}
        </Button>
        {saved ? (
          <span role="status" className="text-sm text-green-700">
            Saved
          </span>
        ) : null}
      </div>
    </form>
  );
}

function modelHint(kind: string, count: number | undefined, isError: boolean): string {
  if (kind !== "hosted") return "Models apply to hosted agents.";
  if (isError) return "Could not load models.";
  if (count === 0) return "Available when an LLM key is granted.";
  return "Only models from usable keys are listed.";
}
