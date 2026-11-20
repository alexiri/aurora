"use client";

import { useState, type FormEvent } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { simWords } from "@/lib/labels";
import { Section } from "@/components/ui/section";
import { useSimulate, type Simulation } from "@/lib/api/policy";
import { fmtSecs } from "@/lib/approval-limits";
import type { Agent } from "@/lib/api/queries";

function TraceList({ title, trace }: { title: string; trace: Simulation["trace"]["workspace"] }) {
  return (
    <section aria-label={`${title} trace`} className="space-y-1">
      <h4 className="text-sm font-medium text-ink">
        {title}: <span data-testid={`${title}-verdict`}>{trace.verdict ?? "no decision"}</span>
      </h4>
      {trace.considered.length === 0 ? (
        <p className="text-xs text-muted">No rules considered.</p>
      ) : (
        <ul className="space-y-1 text-xs">
          {trace.considered.map((c, i) => (
            <li key={`${c.rule_logical_id}-${i}`} className={c.matched ? "font-medium text-ink" : "text-ink-2"}>
              {c.matched ? "Matched" : "Skipped"} · {c.rule_name} ({c.source}) — {c.reason}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** The rule that decided: the matched entry of the workspace trace, else the organization's. */
function decidingRule(sim: Simulation): string | null {
  const hit = [...sim.trace.workspace.considered, ...sim.trace.organization.considered].find((c) => c.matched && c.source !== "default");
  return hit?.rule_name ?? null;
}

/**
 * (P5-REFINE-16, workspace-1) The answer in a sentence, then the explanation, then the raw verdict, guidance code and winning
 * credential collapsed under Technical details.
 */
export function SimulationResult({ sim, agentName = "The agent" }: { sim: Simulation; agentName?: string }) {
  const words = simWords(sim, decidingRule(sim), agentName);
  return (
    <div className="space-y-3" aria-label="Result">
      <Notice tone={sim.verdict === "allow" ? "accent" : sim.verdict === "block" ? "danger" : "pending"} className="space-y-1" data-testid="sim-answer">
        <p className="text-sm">
          <strong>{words.lead}</strong>
          {words.before}
          {words.ruleName ? <strong>{words.ruleName}</strong> : null}
          {words.after}
        </p>
        {words.second ? <p className="text-xs">{words.second}</p> : null}
      </Notice>
      {sim.verdict === "needs_approval" && (sim.approval_parks || sim.approval_timeout_clamped) ? (
        // (P4-011B-4, C-829) What the approval would do: park (the agent stops waiting) and/or a timeout cut to the maximum.
        <p className="text-sm" data-testid="approval-timing">
          {sim.approval_parks ? <span>Parks: the agent stops waiting and is told when a person decides. </span> : null}
          {sim.approval_timeout_clamped ? (
            <span>Timeout clamped to {fmtSecs(sim.approval_timeout_secs ?? 0)}, the instance maximum.</span>
          ) : null}
        </p>
      ) : null}
      {sim.app ? (
        <p className="text-sm" data-testid="recognized-as">
          Recognized as: <strong>{sim.app.app_id}</strong>
          {" · "}
          {sim.app.tool_id ? (
            <code className="font-mono">{sim.app.tool_id}</code>
          ) : (
            <span className="text-ink-2">an action that is not in the app&apos;s list</span>
          )}
          {sim.winning_credential ? <span>, with the account {sim.winning_credential.name}</span> : null}
        </p>
      ) : null}
      <TechnicalDetails bodyClassName="space-y-3 text-ink" data-testid="sim-technical">
        <p className="text-sm">
          Verdict: <strong data-testid="verdict">{sim.verdict}</strong>
          {sim.reason ? <span className="text-ink-2"> ({sim.reason})</span> : null}
        </p>
        {sim.guidance_code ? (
          <p className="text-sm">
            Guidance: <code className="font-mono">{sim.guidance_code}</code>
          </p>
        ) : null}
        <p className="text-sm">
          Winning credential:{" "}
          {sim.winning_credential ? (
            <span>
              {sim.winning_credential.name} ({sim.winning_credential.kind})
            </span>
          ) : (
            <span className="text-ink-2">none</span>
          )}
        </p>
        <TraceList title="Organization" trace={sim.trace.organization} />
        <TraceList title="Workspace" trace={sim.trace.workspace} />
      </TechnicalDetails>
    </div>
  );
}

const NO_SPEAKER = "none";

/**
 * (C-926, D-407) **Try a request** (check what would happen): the agent, the method and the address are the question; the speaker, the lane, the
 * body and whether to include the changes not applied yet sit under **More**. FIX-159's result text is unchanged.
 */
export function SimulatePanel({
  wsId,
  agents,
  users = [],
  currentUserId,
  onDone,
}: {
  onDone?: () => void;
  wsId: string;
  agents: Agent[];
  users?: { id: string; label: string }[];
  currentUserId?: string;
}) {
  const sim = useSimulate(wsId);
  // RFE-005: the speaker whose turn is simulated; default you, or "no speaker" (schedules,
  // webhooks, BYO and boot traffic), for which `user` rules never match.
  const [speaker, setSpeaker] = useState<string | null>(null);
  const speakerValue = speaker ?? (currentUserId && users.some((u) => u.id === currentUserId) ? currentUserId : NO_SPEAKER);
  const speakerId = speakerValue === NO_SPEAKER ? null : speakerValue;
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
        : {
            agent_id: agentId,
            method,
            url: url.trim(),
            body: body || null,
            use_draft: useDraft,
            lane: "request",
            speaker_user_id: speakerId,
          },
    );
  }

  return (
    <Section id="policy-simulate" title="Try a request" description="Check what would happen: try a request against the rules, with or without the changes not applied yet.">
      <div className="space-y-4">
        <form onSubmit={submit} className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
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
          <div className="flex flex-wrap gap-2">
            <div className="w-28">
              <Field id="sim-method" label="Method">
                <select id="sim-method" className={inputClass} value={method} disabled={tunnel} onChange={(e) => setMethod(e.target.value)}>
                  {["GET", "POST", "PUT", "PATCH", "DELETE"].map((m) => (
                    <option key={m}>{m}</option>
                  ))}
                </select>
              </Field>
            </div>
            <div className="min-w-0 flex-1 basis-48">
              <Field id="sim-url" label="URL">
                <input id="sim-url" className={inputClass} value={url} onChange={(e) => setUrl(e.target.value)} />
              </Field>
            </div>
          </div>
          <details className="space-y-4">
            <summary className="cursor-pointer text-sm font-medium text-ink">More</summary>
            <div className="mt-3 space-y-4">
              {tunnel ? (
                <p className="text-xs text-muted">Browser connections carry no speaker, so rules naming a person never match them.</p>
              ) : (
                <Field
                  id="sim-speaker"
                  label="Speaker"
                  hint="Rules naming a person match only turns that person speaks. “No speaker” covers schedules, webhooks, BYO and boot traffic."
                >
                  <select id="sim-speaker" className={inputClass} value={speakerValue} onChange={(e) => setSpeaker(e.target.value)}>
                    <option value={NO_SPEAKER}>No speaker</option>
                    {users.map((u) => (
                      <option key={u.id} value={u.id}>
                        {u.id === currentUserId ? `${u.label} — you` : u.label}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              <fieldset className="flex flex-wrap gap-x-4 gap-y-1 text-sm text-ink">
                <legend className="mb-1 font-medium">Lane</legend>
                <label className="flex items-center gap-1.5">
                  <input type="radio" name="sim-lane" className="size-4 accent-accent" checked={!tunnel} onChange={() => setTunnel(false)} />
                  Request
                </label>
                <label className="flex items-center gap-1.5">
                  <input type="radio" name="sim-lane" className="size-4 accent-accent" checked={tunnel} onChange={() => setTunnel(true)} />
                  Browser connection (https://host[:port])
                </label>
              </fieldset>
              <Field id="sim-body" label="Body (optional)">
                <textarea id="sim-body" rows={3} className={inputClass} value={body} onChange={(e) => setBody(e.target.value)} />
              </Field>
              <label className="flex items-center gap-2 text-sm text-ink">
                <input type="checkbox" className="size-4 accent-accent" checked={useDraft} onChange={(e) => setUseDraft(e.target.checked)} />
                Include changes not applied yet
              </label>
            </div>
          </details>
          <Button type="submit" size="lg" disabled={!agentId || sim.isPending}>
            {sim.isPending ? "Checking…" : "Check"}
          </Button>
        </form>
        {sim.isError ? <ErrorState error={sim.error} /> : null}
        {sim.data ? <SimulationResult sim={sim.data} agentName={agents.find((a) => a.id === agentId)?.name} /> : null}
        {sim.data ? (
          <div className="flex justify-end gap-2">
            <Button type="button" variant="outline" onClick={() => sim.reset()}>
              Check another
            </Button>
            {onDone ? (
              <Button type="button" onClick={onDone}>
                Done
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
    </Section>
  );
}
