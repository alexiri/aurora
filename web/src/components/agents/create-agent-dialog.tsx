"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { useCreateAgent } from "@/lib/api/agents";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useInstance, useWorkspace } from "@/lib/api/queries";

import { AgentRevealDialog, type Revealed } from "./reveal";

const HANDLE_RE = /^[a-z][a-z0-9-]{0,39}$/;

export function CreateAgentDialog({ wsId }: { wsId: string }) {
  const instance = useInstance();
  const hostedAllowed = instance.data?.features.hosted === true;
  const create = useCreateAgent(wsId);
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
  const [handle, setHandle] = useState("");
  const [kind, setKind] = useState<"byo" | "hosted">("byo");
  const [brief, setBrief] = useState("");
  const [clientErr, setClientErr] = useState<Record<string, string>>({});
  const [revealed, setRevealed] = useState<Revealed | null>(null);
  const fe = create.error ? fieldErrors(create.error) : {};

  function reset() {
    setName("");
    setHandle("");
    setKind("byo");
    setBrief("");
    setClientErr({});
    create.reset();
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    const errs: Record<string, string> = {};
    const n = name.trim();
    if (!n || n.length > 80) errs.name = "Name must be 1 to 80 characters.";
    const h = handle.trim();
    if (h && !HANDLE_RE.test(h))
      errs.handle = "Use lowercase letters, digits and dashes, starting with a letter (max 40).";
    setClientErr(errs);
    if (Object.keys(errs).length) return;
    try {
      const res = await create.mutateAsync({
        name: n,
        handle: h || undefined,
        kind: byoBlocked && hostedAllowed ? "hosted" : kind,
        brief: brief.trim() || undefined,
      });
      setOpen(false);
      reset();
      if (res.token) {
        setRevealed({ token: res.token, config: res.container_config ?? null, agentName: res.agent.name });
      }
    } catch {
      // surfaced via create.error
    }
  }

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          if (!o) reset();
        }}
      >
        <DialogTrigger asChild>
          <Button>New agent</Button>
        </DialogTrigger>
        <DialogContent title="New agent">
          <form onSubmit={(e) => void submit(e)} noValidate className="space-y-3">
            <Field id="agent-name" label="Name" error={clientErr.name || fe.name}>
              <input id="agent-name" className={inputClass} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
            </Field>
            <Field
              id="agent-handle"
              label="Handle (optional)"
              hint="Cannot be changed later."
              error={clientErr.handle || fe.handle}
            >
              <input id="agent-handle" className={inputClass} value={handle} onChange={(e) => setHandle(e.target.value)} />
            </Field>
            <Field id="agent-kind" label="Kind">
              <select
                id="agent-kind"
                className={inputClass}
                value={byoBlocked && hostedAllowed ? "hosted" : kind}
                onChange={(e) => setKind(e.target.value as "byo" | "hosted")}
              >
                <option value="byo" disabled={byoBlocked !== null}>
                  Bring your own (BYO){byoBlocked ? " (owners only)" : ""}
                </option>
                {hostedAllowed ? <option value="hosted">Hosted</option> : null}
              </select>
            </Field>
            {byoBlocked ? (
              <p role="status" className="text-sm text-muted">
                {byoBlocked}.
              </p>
            ) : null}
            <Field id="agent-brief" label="Brief (optional)" error={fe.brief}>
              <textarea id="agent-brief" className={inputClass} rows={3} value={brief} onChange={(e) => setBrief(e.target.value)} />
            </Field>
            {create.error && Object.keys(fe).length === 0 ? (
              <p role="alert" className="text-sm text-red-700">
                {errorMessage(create.error)}
              </p>
            ) : null}
            <Button type="submit" disabled={create.isPending || (byoBlocked !== null && !hostedAllowed)}>
              {create.isPending ? "Creating…" : "Create agent"}
            </Button>
          </form>
        </DialogContent>
      </Dialog>
      <AgentRevealDialog revealed={revealed} onClose={() => setRevealed(null)} />
    </>
  );
}
