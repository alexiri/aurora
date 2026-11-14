"use client";

import Link from "next/link";
import { useState } from "react";

import { Button, buttonVariants } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import {
  useCreateSpaceBinding,
  useIntegrations,
  useMySpaces,
  type Listen,
  type Presence,
} from "@/lib/api/channels";
import { ApiError } from "@/lib/api/client";
import { cn } from "@/lib/utils";

import { dmUrl } from "./connected-done";
import { MutationError } from "./shared";

/** A shared presence that can be talked to: the only kind with a bot every space can invite. */
export function isTalkable(p: Presence): boolean {
  return p.mode === "shared" && p.status === "active";
}

/** The refusals of `POST /v1/agents/{agent}/space-bindings` (C-713) that name a next action. */
function BindError({ error }: { error: unknown }) {
  if (!error) return null;
  if (error instanceof ApiError && error.code === "link_required") {
    return (
      <Notice tone="pending" role="alert" className="space-y-2">
        <p>Link your chat account first, so Aurora knows which channels you are in.</p>
        <Link href="/account/channels" className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          Link your chat account
        </Link>
      </Notice>
    );
  }
  return <MutationError error={error} />;
}

/**
 * "Add it to a channel": the channels the person is in where the bot already is
 * (`GET /v1/integrations/{id}/spaces?mine=true`), bound with `POST …/space-bindings` (C-713/C-714).
 */
function ChannelPicker({ agentId, integrationId, botLabel }: { agentId: string; integrationId: string; botLabel: string }) {
  const spaces = useMySpaces(integrationId, true);
  const create = useCreateSpaceBinding(agentId);
  const [space, setSpace] = useState("");
  const [listen, setListen] = useState<Listen>("mentions");

  const items = spaces.data?.items ?? [];
  const bound = create.isSuccess ? items.find((s) => s.space_ref === create.variables?.space_ref) : undefined;

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!space) return;
    create.mutate({ integration_id: integrationId, space_ref: space, listen });
  }

  return (
    <form className="space-y-3" onSubmit={submit} aria-busy={create.isPending}>
      {spaces.isPending ? <p className="text-sm text-muted">Looking for the channels you are in…</p> : null}
      {spaces.isError ? <BindError error={spaces.error} /> : null}
      {spaces.isSuccess && items.length === 0 ? (
        <Notice tone="neutral" role="status">
          No channel to show yet. Invite {botLabel} to a channel you are in (a private channel too), then reload this list.
        </Notice>
      ) : null}
      {items.length ? (
        <>
          <Field id="bind-space" label="Channel">
            <select id="bind-space" className={inputClass} value={space} onChange={(e) => setSpace(e.target.value)}>
              <option value="">Select…</option>
              {items.map((s) => (
                <option key={s.space_ref} value={s.space_ref}>
                  {s.kind === "private" ? "Private · " : "# "}
                  {s.label}
                </option>
              ))}
            </select>
          </Field>
          <Field id="bind-listen" label="Listen">
            <select id="bind-listen" className={inputClass} value={listen} onChange={(e) => setListen(e.target.value as Listen)}>
              <option value="mentions">Only when mentioned</option>
              <option value="all">Every message</option>
            </select>
          </Field>
        </>
      ) : null}
      {bound ? (
        <Notice tone="accent" role="status">
          Bound. {botLabel} announced itself in {bound.label}.
        </Notice>
      ) : null}
      <BindError error={create.error} />
      <div className="flex flex-wrap gap-2">
        {items.length ? (
          <Button type="submit" disabled={!space || create.isPending}>
            {create.isPending ? "Adding…" : "Add to this channel"}
          </Button>
        ) : null}
        <Button type="button" variant="outline" disabled={spaces.isFetching} onClick={() => void spaces.refetch()}>
          {spaces.isFetching ? "Checking…" : "Reload channels"}
        </Button>
      </div>
      <p className="text-xs text-muted">Only channels you are a member of are listed. Or write “{botLabel} bind &lt;handle&gt;” in the channel.</p>
    </form>
  );
}

/**
 * (FIX-127 Part C) What to do after attaching: talk to the bot in a DM, or invite it to a channel and
 * bind it here. Shown for an active shared presence, to people who own the agent.
 */
export function NextSteps({ agentId, orgId, presence }: { agentId: string; orgId?: string; presence: Presence }) {
  // The owner-only list may be refused to a workspace owner who is not an org admin: then no DM link.
  const integrations = useIntegrations(orgId);
  const integration = integrations.data?.items.find((i) => i.id === presence.integration_id);
  const bot = (presence.handle ?? integration?.shared_bot?.handle ?? "").replace(/^@/, "");
  const config = (integration?.config ?? {}) as Record<string, unknown>;
  const dm = dmUrl(
    typeof config.server_url === "string" ? config.server_url : undefined,
    typeof config.team === "string" ? config.team : undefined,
    bot,
  );
  const botLabel = bot ? `@${bot}` : "the bot";
  const [open, setOpen] = useState(false);
  return (
    <section
      aria-labelledby={`next-${presence.id}`}
      className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5"
      data-testid="next-steps"
    >
      <h3 id={`next-${presence.id}`} className="font-display text-base font-semibold text-ink">
        Next steps for {presence.tenant_label}
      </h3>
      <div className="space-y-1.5">
        <h4 className="text-sm font-semibold text-ink">Talk to it</h4>
        {dm ? (
          <a href={dm} target="_blank" rel="noopener noreferrer" className={cn(buttonVariants({ variant: "outline" }))}>
            Open a DM with {botLabel}
          </a>
        ) : (
          <p className="text-sm text-ink-2">Open a direct message with {botLabel}.</p>
        )}
      </div>
      <div className="space-y-2">
        <h4 className="text-sm font-semibold text-ink">Add it to a channel</h4>
        <p className="text-sm text-ink-2">Invite {botLabel} to the channel first, then bind it here.</p>
        {open ? (
          <ChannelPicker agentId={agentId} integrationId={presence.integration_id} botLabel={botLabel} />
        ) : (
          <Button variant="outline" onClick={() => setOpen(true)}>
            Add to a channel
          </Button>
        )}
      </div>
    </section>
  );
}
