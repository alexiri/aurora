"use client";

import { useState, type FormEvent } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { useSimulate, type Simulation } from "@/lib/api/policy";
import type { Agent } from "@/lib/api/queries";

function TraceList({ title, trace }: { title: string; trace: Simulation["trace"]["workspace"] }) {
  return (
    <section aria-label={`${title} trace`} className="space-y-1">
      <h4 className="text-sm font-medium">
        {title}: <span data-testid={`${title}-verdict`}>{trace.verdict ?? "no decision"}</span>
      </h4>
      {trace.considered.length === 0 ? (
        <p className="text-xs text-neutral-500">No rules considered.</p>
      ) : (
        <ul className="space-y-1 text-xs">
          {trace.considered.map((c, i) => (
            <li key={`${c.rule_logical_id}-${i}`} className={c.matched ? "font-medium" : "text-neutral-600"}>
              {c.matched ? "Matched" : "Skipped"} · {c.rule_name} ({c.source}) — {c.reason}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export function SimulationResult({ sim }: { sim: Simulation }) {
  return (
    <div className="space-y-3 rounded border border-neutral-200 p-3" aria-label="Simulation result">
      <p className="text-sm">
        Verdict: <strong data-testid="verdict">{sim.verdict}</strong>
        {sim.reason ? <span className="text-neutral-600"> ({sim.reason})</span> : null}
      </p>
      {sim.guidance_code ? (
        <p className="text-sm">
          Guidance: <code>{sim.guidance_code}</code>
        </p>
      ) : null}
      <p className="text-sm">
        Winning credential:{" "}
        {sim.winning_credential ? (
          <span>
            {sim.winning_credential.name} ({sim.winning_credential.kind})
          </span>
        ) : (
          <span className="text-neutral-600">none</span>
        )}
      </p>
      <TraceList title="Organization" trace={sim.trace.organization} />
      <TraceList title="Workspace" trace={sim.trace.workspace} />
    </div>
  );
}

export function SimulatePanel({ wsId, agents }: { wsId: string; agents: Agent[] }) {
  const sim = useSimulate(wsId);
  const [agentId, setAgentId] = useState("");
  const [method, setMethod] = useState("GET");
  const [url, setUrl] = useState("https://");
  const [body, setBody] = useState("");
  const [useDraft, setUseDraft] = useState(true);
  const [tunnel, setTunnel] = useState(false);

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!agentId) return;
    sim.mutate(
      tunnel
        ? { agent_id: agentId, method: "CONNECT", url: url.trim(), use_draft: useDraft, lane: "open" }
        : { agent_id: agentId, method, url: url.trim(), body: body || null, use_draft: useDraft, lane: "request" },
    );
  }

  return (
    <section aria-label="Simulate" className="space-y-3">
      <h2 className="text-lg font-medium">Simulate</h2>
      <form onSubmit={submit} className="space-y-3">
        <Field id="sim-agent" label="Agent">
          <select id="sim-agent" className={inputClass} value={agentId} onChange={(e) => setAgentId(e.target.value)}>
            <option value="">Select an agent…</option>
            {agents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name} (@{a.handle})
              </option>
            ))}
          </select>
        </Field>
        <fieldset className="flex gap-4 text-sm">
          <legend className="sr-only">Lane</legend>
          <label className="flex items-center gap-1">
            <input type="radio" name="sim-lane" checked={!tunnel} onChange={() => setTunnel(false)} />
            Request
          </label>
          <label className="flex items-center gap-1">
            <input type="radio" name="sim-lane" checked={tunnel} onChange={() => setTunnel(true)} />
            Tunnel (browser, https://host[:port])
          </label>
        </fieldset>
        <div className="flex gap-2">
          <div className="w-28">
            <Field id="sim-method" label="Method">
              <select id="sim-method" className={inputClass} value={method} onChange={(e) => setMethod(e.target.value)}>
                {["GET", "POST", "PUT", "PATCH", "DELETE"].map((m) => (
                  <option key={m}>{m}</option>
                ))}
              </select>
            </Field>
          </div>
          <div className="flex-1">
            <Field id="sim-url" label="URL">
              <input id="sim-url" className={inputClass} value={url} onChange={(e) => setUrl(e.target.value)} />
            </Field>
          </div>
        </div>
        <Field id="sim-body" label="Body (optional)">
          <textarea id="sim-body" rows={3} className={inputClass} value={body} onChange={(e) => setBody(e.target.value)} />
        </Field>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={useDraft} onChange={(e) => setUseDraft(e.target.checked)} />
          Use saved draft
        </label>
        <Button type="submit" disabled={!agentId || sim.isPending}>
          {sim.isPending ? "Simulating…" : "Simulate"}
        </Button>
      </form>
      {sim.isError ? <ErrorState error={sim.error} /> : null}
      {sim.data ? <SimulationResult sim={sim.data} /> : null}
    </section>
  );
}
