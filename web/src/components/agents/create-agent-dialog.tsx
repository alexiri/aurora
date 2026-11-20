"use client";

import { useParams, useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { ChoiceChips } from "@/components/ui/choice-chips";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { showUndoToast } from "@/components/ui/undo-toast";
import { useCreateAgent } from "@/lib/api/agents";
import { fieldErrors } from "@/lib/api/errors";
import { useInstance, useMe, useWorkspace } from "@/lib/api/queries";
import { AGENT_TEMPLATES, BRIEF_MAX, DRAFT_MAX, fillTemplate, templateFor, type AgentTemplateId } from "@/lib/onboarding/templates";
import { wsRoutes } from "@/lib/routes";

import { AgentRevealDialog, type Revealed } from "./reveal";

const HANDLE_RE = /^[a-z][a-z0-9-]{0,39}$/;

/** A handle made from a name ("Rex the Ops helper" → "rex-the-ops-helper"); empty when nothing usable is left. */
export function handleFromName(name: string): string {
  const h = name
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^[^a-z]+/, "")
    .slice(0, 40)
    .replace(/-+$/, "");
  return HANDLE_RE.test(h) ? h : "";
}

const CHIPS = AGENT_TEMPLATES.map((t) => ({ value: t.id, label: t.label }));

export function CreateAgentDialog({ wsId }: { wsId: string }) {
  const instance = useInstance();
  const hostedAllowed = instance.data?.features.hosted === true;
  const create = useCreateAgent(wsId);
  const router = useRouter();
  const params = useParams<{ org?: string; ws?: string }>();
  const me = useMe();
  // (RFE-025) BYO is offered only as the effective posture allows; unknown = let the server decide.
  const ws = useWorkspace(wsId);
  const posture = ws.data?.effective_byo_posture;
  const byoBlocked: string | null =
    posture?.value === "disabled"
      ? `BYO agents are disabled by the ${posture.level} settings`
      : posture?.value === "owners_only" && ws.data && ws.data.my_role !== "owner"
        ? "BYO agents are for workspace owners only"
        : null;
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [handle, setHandle] = useState<string | null>(null); // null = follows the name
  const [kind, setKind] = useState<"byo" | "hosted">("hosted");
  const [moreOpen, setMoreOpen] = useState(false);
  const [templateId, setTemplateId] = useState<AgentTemplateId>("general");
  const [typed, setTyped] = useState<string | null>(null); // the person's own text; null = the template's
  const [clientErr, setClientErr] = useState<Record<string, string>>({});
  const [revealed, setRevealed] = useState<Revealed | null>(null);
  const [after, setAfter] = useState<string | null>(null);
  const fe = create.error ? fieldErrors(create.error) : {};

  const template = templateFor(templateId);
  const person = me.data?.user.display_name ?? null;
  const shownName = name.trim() || template.nameSuggestion;
  const instructions = typed ?? fillTemplate(template.instructions, { name: shownName, person });
  const handleValue = handle ?? handleFromName(name);
  // Hosted is the default; BYO is the only choice when the instance has no hosted agents.
  const effectiveKind: "byo" | "hosted" = !hostedAllowed ? "byo" : byoBlocked && kind === "byo" ? "hosted" : kind;

  function reset() {
    setName("");
    setHandle(null);
    setKind("hosted");
    setMoreOpen(false);
    setTemplateId("general");
    setTyped(null);
    setClientErr({});
    create.reset();
  }

  function pick(id: AgentTemplateId) {
    setTemplateId(id);
    // A template only fills what is still empty (Name) or still its own (Instructions); it never changes access.
    if (!name.trim()) setName(templateFor(id).nameSuggestion);
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    const errs: Record<string, string> = {};
    const n = name.trim();
    if (!n || n.length > 80) errs.name = "Name must be 1 to 80 characters.";
    const h = handleValue.trim();
    if (h && !HANDLE_RE.test(h))
      errs.handle = "Use lowercase letters, digits and dashes, starting with a letter (max 40).";
    if (instructions.length > BRIEF_MAX) errs.brief = "Instructions are too long.";
    setClientErr(errs);
    if (Object.keys(errs).length) return;
    try {
      const res = await create.mutateAsync({
        name: n,
        handle: h || undefined,
        kind: effectiveKind,
        brief: instructions.trim() || undefined,
      });
      const draft = fillTemplate(template.firstMessage, { name: res.agent.name, person }).slice(0, DRAFT_MAX);
      setOpen(false);
      reset();
      showUndoToast(`${res.agent.name} is ready`);
      const routes = params.org && params.ws ? wsRoutes(params.org, params.ws) : null;
      if (res.token) {
        // A BYO agent opens on Connect once its token has been copied.
        if (routes) setAfter(routes.agent(res.agent.handle, { tab: "connect" }));
        setRevealed({
          token: res.token,
          config: res.container_config ?? null,
          agentName: res.agent.name,
          expiresAt: res.expires_at,
        });
      } else if (routes) {
        router.push(routes.agent(res.agent.handle, { draft }));
      }
    } catch {
      // surfaced via create.error
    }
  }

  return (
    <>
      <Button onClick={() => setOpen(true)}>New agent</Button>
      <FormDialog
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          if (!o) reset();
        }}
        title="New agent"
        submitLabel={create.isPending ? "Creating…" : "Create agent"}
        busy={create.isPending}
        submitDisabled={effectiveKind === "byo" && byoBlocked !== null}
        onSubmit={(e) => void submit(e)}
      >
        <div className="space-y-4">
          <div className="space-y-1.5">
            <p id="agent-template-label" className="text-sm font-medium text-ink">
              Start from
            </p>
            <ChoiceChips label="Start from" value={templateId} options={CHIPS} onChange={pick} />
          </div>
          <Field id="agent-name" label="Name" error={clientErr.name || fe.name}>
            <input
              id="agent-name"
              className={inputClass}
              value={name}
              maxLength={80}
              placeholder={template.nameSuggestion || "Rex"}
              onChange={(e) => setName(e.target.value)}
            />
          </Field>
          <Field id="agent-handle" label="Handle (optional)" hint="Cannot be changed later." error={clientErr.handle || fe.handle}>
            <input id="agent-handle" className={inputClass} value={handleValue} onChange={(e) => setHandle(e.target.value)} />
          </Field>
          {hostedAllowed ? (
            <Field id="agent-kind" label="Where it runs" hint="Aurora hosts the agent and its computer for you.">
              <select
                id="agent-kind"
                className={inputClass}
                value={effectiveKind}
                onChange={(e) => setKind(e.target.value as "byo" | "hosted")}
              >
                <option value="hosted">Runs on Aurora (recommended)</option>
                {moreOpen ? (
                  <option value="byo" disabled={byoBlocked !== null}>
                    Runs on my own computer (bring your own agent){byoBlocked ? " (owners only)" : ""}
                  </option>
                ) : null}
              </select>
            </Field>
          ) : (
            <Notice role="status">
              {byoBlocked
                ? `${byoBlocked}. This instance has no hosted agents.`
                : "This instance has no hosted agents, so this agent is a program you run yourself (bring your own)."}
            </Notice>
          )}
          <Field
            id="agent-brief"
            label="Instructions (optional)"
            error={clientErr.brief || fe.brief}
          >
            <textarea
              id="agent-brief"
              className={inputClass}
              rows={5}
              value={instructions}
              placeholder={`What ${shownName || "Rex"} should do and how.`}
              onChange={(e) => setTyped(e.target.value)}
            />
          </Field>
          {hostedAllowed ? (
            <div className="text-sm text-ink-2">
              <button
                type="button"
                aria-expanded={moreOpen}
                onClick={() => setMoreOpen((o) => !o)}
                className="flex min-h-9 w-full items-start gap-1.5 rounded-control text-left hover:text-ink focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
              >
                <span aria-hidden="true">{moreOpen ? "▾" : "▸"}</span>
                <span>More options: bring your own agent (a program you run yourself; workspace owners only)</span>
              </button>
              {moreOpen ? (
                <p className="mt-1 text-sm text-muted">
                  {byoBlocked ? `${byoBlocked}.` : "Choose “Runs on my own computer” under Where it runs."}
                </p>
              ) : null}
            </div>
          ) : null}
          {create.error && Object.keys(fe).length === 0 ? <ErrorNotice error={create.error} /> : null}
        </div>
      </FormDialog>
      <AgentRevealDialog
        revealed={revealed}
        onClose={() => {
          setRevealed(null);
          if (after) router.push(after);
          setAfter(null);
        }}
      />
    </>
  );
}
