"use client";

import { useIsMutating } from "@tanstack/react-query";
import Link from "next/link";

import { AskAdminForm } from "@/components/ask-admin";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "@/components/ui/menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { showUndoToast } from "@/components/ui/undo-toast";
import { useAddCredentialGrants, useGrantSecretToAgent } from "@/lib/api/credential-grants";
import { useAgentAiKeys, type AgentAiKey } from "@/lib/api/agent-ai-keys";
import { useAnswerReadiness, type AnswerFix } from "@/lib/api/answer-readiness";
import { useMyRequests } from "@/lib/api/app-setup-requests";
import { errorMessage } from "@/lib/api/errors";
import { orgRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/**
 * (P5-REFINE-11 item 8, P5-REFINE-12 item 7, D-414, access-3) What fixes an agent that cannot answer, chosen by the API for
 * the viewer (C-1062 `AnswerReadiness.fix`): **give_key** (one click: "Give Rex the Company key"), **add_key** (a link to the
 * organization's AI keys, for organization owners and admins) or **ask_admin** (the `AskAdmin` control, subject `ai_key`,
 * with its optional reason, C-1081). Used by the header, the empty chat and the composer.
 */
export function useAnswerFix(agentId: string, enabled = true) {
  const q = useAnswerReadiness(agentId, enabled);
  const d = q.data;
  return {
    cantAnswer: d?.can_answer === false,
    /** The agent has no AI key at all: "<Agent> has no AI model yet". */
    noKey: d?.can_answer === false && d.reason === "no_ai_key",
    fix: d?.fix ?? null,
  };
}

/** `/o/<org>/w/<ws>` → the organization's slug (the workspace's own path is all the chat knows). */
export function orgSlugOf(base: string): string | null {
  const m = /^\/o\/([^/]+)/.exec(base);
  return m ? decodeURIComponent(m[1]!) : null;
}

/**
 * One key for every "give this agent a key" mutation, so the one-click button and the **Use another key** menu (in every
 * place that shows them) wait for each other: two clicks must not grant two keys.
 */
const keyGrantMutationKey = (agentId: string) => ["ai-key-grant", agentId] as const;

function useKeyGrantPending(agentId: string): boolean {
  return useIsMutating({ mutationKey: keyGrantMutationKey(agentId) }) > 0;
}

/** The key's scope in words (FIX-249, C-1168): "Organization key" | "Workspace key". */
export function scopeWords(k: Pick<AgentAiKey, "scope">): string {
  return k.scope === "workspace" ? "Workspace key" : "Organization key";
}

/**
 * (FIX-249, C-1168) The quiet "Use another key" control beside the one-click give — one shared
 * component for every place that offers the fix (the header, the empty chat, the starter-key
 * card). Shown when the agent's organization or workspace holds more than one key the agent does
 * not have yet: the other usable keys grant in one click with the same toast ("<Agent> can answer
 * now"), the keys that cannot be used are disabled rows with the reason in plain words (C-1168).
 */
export function UseAnotherKey({ agentId, agentName, given, size = "sm" }: { agentId: string; agentName: string; given: string; size?: "sm" | "default" }) {
  const keys = useAgentAiKeys(agentId);
  const grant = useGrantSecretToAgent(keyGrantMutationKey(agentId));
  const pending = useKeyGrantPending(agentId);
  const items = keys.data?.items ?? [];
  const others = items.filter((k) => k.usable && k.secret_id !== given);
  const unusable = items.filter((k) => !k.usable);
  if (others.length === 0 && unusable.length === 0) return null;
  return (
    <span className="flex min-w-0 flex-col items-start gap-1">
      <Menu>
        <MenuTrigger asChild>
          <Button type="button" size={size} variant="ghost" disabled={pending} className="min-h-11 md:min-h-9">
            Use another key
          </Button>
        </MenuTrigger>
        <MenuContent align="start" className="min-w-56">
          {others.map((k) => (
            <MenuItem
              key={k.secret_id}
              className="flex-col items-stretch gap-0.5"
              onSelect={() => grant.mutate({ secretId: k.secret_id, agentId }, { onSuccess: () => showUndoToast(`${agentName} can answer now`) })}
            >
              <span className="break-words font-medium text-ink">{k.name}</span>
              <span className="text-xs text-muted">{scopeWords(k)}</span>
            </MenuItem>
          ))}
          {others.length > 0 && unusable.length > 0 ? <MenuSeparator /> : null}
          {unusable.map((k) => (
            <MenuItem key={k.secret_id} disabled className="flex-col items-stretch gap-0.5">
              <span className="break-words font-medium text-ink">{k.name}</span>
              <span className="text-xs text-muted">{k.reason}</span>
            </MenuItem>
          ))}
        </MenuContent>
      </Menu>
      {grant.isError ? (
        <span role="alert" className="text-xs text-danger-ink">
          {errorMessage(grant.error)}
        </span>
      ) : null}
    </span>
  );
}

/** "Give Rex the Company key": one click, then "Rex can answer now" (the header turns Ready: the grant refreshes the answer check). */
export function GiveKeyButton({ agentId, agentName, fix, size = "sm", buttonClassName }: { agentId: string; agentName: string; fix: Extract<AnswerFix, { action: "give_key" }>; size?: "sm" | "default"; buttonClassName?: string }) {
  const add = useAddCredentialGrants({ kind: "secret", id: fix.key.secret_id }, keyGrantMutationKey(agentId));
  const pending = useKeyGrantPending(agentId);
  return (
    <span className="flex w-full min-w-0 max-w-full flex-col items-start gap-1">
      <span className="flex w-full min-w-0 max-w-full flex-wrap items-start gap-2">
        <Button
          type="button"
          size={size}
          className={cn("h-auto min-h-11 max-w-full whitespace-normal py-1.5 text-left md:min-h-9", buttonClassName)}
          disabled={pending}
          onClick={() => add.mutate({ agentIds: [agentId] }, { onSuccess: () => showUndoToast(`${agentName} can answer now`) })}>
          Give {agentName} the {fix.key.name}
        </Button>
        <UseAnotherKey agentId={agentId} agentName={agentName} given={fix.key.secret_id} size={size} />
      </span>
      {/* A phrase, not a block: the header's state line is a paragraph. */}
      {add.isError ? (
        <span role="alert" className="text-xs text-danger-ink">
          {errorMessage(add.error)}
        </span>
      ) : null}
    </span>
  );
}

/** "Ask an admin" for an AI key: a button that opens the optional reason, or the asked state once it is sent. */
function AskInPopover({ wsId, agentName }: { wsId: string; agentName: string }) {
  const mine = useMyRequests(wsId);
  const asked = mine.data?.some((r) => r.subject === "ai_key");
  if (asked) return <AskAdminForm workspaceId={wsId} subject="ai_key" />;
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button type="button" size="sm" variant="outline">
          Ask an admin
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-80 max-w-[90vw]">
        <AskAdminForm workspaceId={wsId} subject="ai_key" placeholder={`So ${agentName} can answer`} />
      </PopoverContent>
    </Popover>
  );
}

/** The header's action next to "Can't answer: …" (access-3). */
export function AnswerFixAction({ agentId, agentName, wsId, base }: { agentId: string; agentName: string; wsId: string; base: string }) {
  const { cantAnswer, fix } = useAnswerFix(agentId);
  const org = orgSlugOf(base);
  if (!cantAnswer || !fix) return null;
  if (fix.action === "give_key") return <GiveKeyButton agentId={agentId} agentName={agentName} fix={fix} />;
  if (fix.action === "add_key") {
    return org ? (
      <Link href={orgRoutes(org).accounts({ kind: "ai-key" })} className={buttonVariants({ size: "sm" })}>
        Add a key
      </Link>
    ) : null;
  }
  return <AskInPopover wsId={wsId} agentName={agentName} />;
}

/**
 * (D-414, access-3) The conversation's empty state when the agent has **no AI key at all**: "<Agent> has no AI model yet" with one
 * action, and the same line as a disabled composer once the conversation has messages. It replaces the chat error "Open AI keys,
 * Agent access, Fix it in the guide".
 */
export function NoModelCard({ agentId, agentName, wsId, base }: { agentId: string; agentName: string; wsId: string; base: string }) {
  const { noKey, fix } = useAnswerFix(agentId);
  const org = orgSlugOf(base);
  if (!noKey || !fix) return null;
  return (
    <Card role="region" aria-label={`${agentName} has no AI model yet`} data-testid="no-model-card" className="mx-auto w-full max-w-md space-y-3 text-left">
      <h3 className="font-display text-lg font-semibold text-ink">{agentName} has no AI model yet</h3>
      <p className="text-sm text-ink-2">
        {fix.action === "give_key"
          ? `${agentName} can't answer until it has an AI key. Your organization already has one it can use.`
          : fix.action === "add_key"
            ? `${agentName} can't answer until you add an AI key.`
            : "Only organization owners and admins can add an AI key."}
      </p>
      {fix.action === "give_key" ? <GiveKeyButton agentId={agentId} agentName={agentName} fix={fix} size="default" /> : null}
      {fix.action === "add_key" && org ? (
        <Link href={orgRoutes(org).accounts({ kind: "ai-key" })} className={buttonVariants({})}>
          Add a key
        </Link>
      ) : null}
      {fix.action === "ask_admin" ? <AskAdminForm workspaceId={wsId} subject="ai_key" placeholder={`So ${agentName} can answer`} /> : null}
    </Card>
  );
}

/** The composer's disabled state: one line saying why, so nothing can be typed into the void. */
export function NoModelBar({ agentId, agentName }: { agentId: string; agentName: string }) {
  const { noKey } = useAnswerFix(agentId);
  if (!noKey) return null;
  return (
    <div className="shrink-0 border-t border-line p-3 md:px-5" data-testid="no-model-bar">
      <div aria-disabled="true" className="flex min-h-11 items-center rounded-card border border-line bg-subtle px-3 text-sm text-muted">
        {agentName} has no AI model yet, so it can&apos;t answer.
      </div>
    </div>
  );
}
