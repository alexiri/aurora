"use client";

// Account › Chat platform links › Send as me (phase 5, P4-UTOK, D-281..D-284, C-853..C-855, docs/14 §Account): the
// person's own chat-platform tokens, the agents they let post as them, and Revoke. The token is typed into a masked
// field, sent once and never shown again; agents post only after the person approves each post.
import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";
import { CheckboxField } from "@/components/ui/form";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { errorMessage } from "@/lib/api/errors";
import type { UserLink } from "@/lib/api/channels";
import {
  useAddUserToken,
  useRevokeUserToken,
  useSetUserTokenGrant,
  useUserTokens,
  userTokenErrorText,
  type UserToken,
} from "@/lib/api/user-tokens";

const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleString() : "Never");

export const SEND_AS_ME_HELP =
  "In Mattermost, open Profile › Security › Personal Access Tokens and create one. Aurora stores it sealed and uses it only to post as you, after you approve each post.";

type AgentOption = { id: string; label: string };

/** One chat account per integration the person has linked (a token is checked against it). */
function integrationsOf(links: UserLink[]): { id: string; label: string }[] {
  const seen = new Map<string, string>();
  for (const l of links) if (!seen.has(l.integration_id)) seen.set(l.integration_id, l.tenant_label);
  return [...seen].map(([id, label]) => ({ id, label }));
}

function AddTokenDialog({
  open,
  onOpenChange,
  integrations,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  integrations: { id: string; label: string }[];
}) {
  const add = useAddUserToken();
  const [integration, setIntegration] = useState("");
  const [token, setToken] = useState("");
  const [label, setLabel] = useState("");
  const chosen = integration || integrations[0]?.id || "";
  const reset = () => {
    setIntegration("");
    setToken("");
    setLabel("");
    add.reset();
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    add.mutate(
      { integration_id: chosen, token: token.trim(), label: label.trim() || undefined },
      {
        onSuccess: () => {
          reset();
          onOpenChange(false);
        },
        // The value is never kept in the form after an attempt: type it again.
        onError: () => setToken(""),
      },
    );
  };
  const text = add.error ? (userTokenErrorText(add.error) ?? errorMessage(add.error)) : undefined;
  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
      title="Add token"
      description={SEND_AS_ME_HELP}
      submitLabel="Add token"
      busy={add.isPending}
      submitDisabled={token.trim() === "" || chosen === ""}
      onSubmit={submit}
    >
      {integrations.length > 1 ? (
        <Field id="user-token-integration" label="Chat workspace">
          <Select id="user-token-integration" value={chosen} onChange={setIntegration} options={integrations.map((i) => ({ value: i.id, label: i.label }))} />
        </Field>
      ) : null}
      <Field id="user-token-value" label="Personal access token" error={text}>
        <SecretInput
          id="user-token-value"
          className="font-mono"
          revealLabel="personal access token"
          value={token}
          maxLength={256}
          onChange={setToken}
        />
      </Field>
      <Field id="user-token-label" label="Name (optional)" hint="Shown in this list.">
        <input
          id="user-token-label"
          className={inputClass}
          value={label}
          maxLength={80}
          autoComplete="off"
          onChange={(e) => setLabel(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}

function GrantDialog({
  token,
  agents,
  onClose,
}: {
  token: UserToken | null;
  agents: AgentOption[];
  onClose: () => void;
}) {
  const grant = useSetUserTokenGrant();
  const granted = new Set((token?.agents ?? []).map((a) => a.id));
  return (
    <Dialog
      open={token !== null}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
    >
      <DialogContent
        title="Grant agents"
        description="Choose the agents that may ask to post as you. Each post still waits for your approval."
      >
        {agents.length === 0 ? (
          <p className="text-sm text-ink-2">You can use no agents yet.</p>
        ) : (
          <ul aria-label="Agents" className="max-h-72 space-y-3 overflow-y-auto">
            {agents.map((a) => (
              <li key={a.id}>
                <CheckboxField
                  id={`grant-${a.id}`}
                  label={a.label}
                  checked={granted.has(a.id)}
                  disabled={grant.isPending || token?.status !== "active"}
                  onChange={(checked) => token && grant.mutate({ id: token.id, agent: a.id, granted: checked })}
                />
              </li>
            ))}
          </ul>
        )}
        <ErrorNotice error={grant.error} className="mt-3" />
        <div className="mt-4 flex justify-end">
          <DialogClose asChild>
            <Button variant="outline">Done</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function TokenRow({
  t,
  integrationLabel,
  onGrant,
  onRevoke,
}: {
  t: UserToken;
  integrationLabel: string;
  onGrant: () => void;
  onRevoke: () => void;
}) {
  const name = t.label ?? t.platform_user_label;
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm">
        <span className="min-w-0 break-words font-medium text-ink">{integrationLabel}</span>
        <code className="min-w-0 break-all font-mono text-xs text-ink-2">{t.platform_user_label}</code>
        {t.label ? <span className="min-w-0 break-words text-xs text-muted">{t.label}</span> : null}
        <Badge tone={t.status === "active" ? "accent" : t.status === "invalid" ? "danger" : "neutral"}>{t.status}</Badge>
      </div>
      {t.status === "invalid" ? (
        <Notice tone="danger">
          The chat platform rejected this token. Revoke it and add a new one before agents can post as you again.
        </Notice>
      ) : null}
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="text-xs text-muted">Agents</span>
        {t.agents.length === 0 ? (
          <span className="text-xs text-ink-2">None yet</span>
        ) : (
          <ul aria-label={`Agents granted ${name}`} className="flex flex-wrap gap-1.5">
            {t.agents.map((a) => (
              <li key={a.id} className="rounded-full bg-subtle px-2 py-0.5 text-xs text-ink-2">
                {a.name}
              </li>
            ))}
          </ul>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted">Last used {fmtDate(t.last_used_at)}</span>
        <div className="ml-auto flex flex-wrap gap-2">
          <Button
            variant="outline"
            size="sm"
            aria-label={`Grant agents for ${name}`}
            disabled={t.status !== "active"}
            onClick={onGrant}
          >
            Grant agents
          </Button>
          <Button variant="outline" size="sm" aria-label={`Revoke ${name}`} onClick={onRevoke}>
            Revoke
          </Button>
        </div>
      </div>
    </li>
  );
}

/**
 * The "Send as me" section. `links` are the person's chat accounts (a token is verified against one of them);
 * `agents` are the agents the person can use (the grant switches).
 */
export function SendAsMeSection({ links, agents }: { links: UserLink[]; agents: AgentOption[] }) {
  const tokens = useUserTokens();
  const revoke = useRevokeUserToken();
  const [adding, setAdding] = useState(false);
  const [granting, setGranting] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<UserToken | null>(null);
  const integrations = integrationsOf(links);

  if (tokens.isPending) return <Loading />;
  if (tokens.isError) return <ErrorState error={tokens.error} onRetry={() => tokens.refetch()} />;

  const workspaceName = (t: UserToken) =>
    integrations.find((i) => i.id === t.integration.id)?.label ?? t.integration.type;
  const grantTarget = tokens.data.find((t) => t.id === granting) ?? null;
  const addButton = (
    <Button onClick={() => setAdding(true)} disabled={integrations.length === 0}>
      Add token
    </Button>
  );
  return (
    <>
      <Section
        id="send-as-me"
        title="Send as me"
        description="Let agents you choose post in chat under your own account, after you approve each post. Every post ends with a line that says an agent wrote it."
        actions={tokens.data.length > 0 ? addButton : undefined}
      >
        <div className="space-y-3">
          <ErrorNotice error={revoke.error} />
          {tokens.data.length === 0 ? (
            <EmptyState
              title="Agents post as themselves unless you add a token."
              when={
                integrations.length === 0
                  ? "Link a chat account first: message the bot in your chat platform to get a sign-in link."
                  : undefined
              }
              action={integrations.length > 0 ? addButton : undefined}
            />
          ) : (
            <ul aria-label="Send as me tokens" className="space-y-3">
              {tokens.data.map((t) => (
                <TokenRow
                  key={t.id}
                  t={t}
                  integrationLabel={workspaceName(t)}
                  onGrant={() => setGranting(t.id)}
                  onRevoke={() => setRevoking(t)}
                />
              ))}
            </ul>
          )}
        </div>
      </Section>
      <AddTokenDialog open={adding} onOpenChange={setAdding} integrations={integrations} />
      <GrantDialog token={grantTarget} agents={agents} onClose={() => setGranting(null)} />
      <ConfirmDialog
        open={revoking !== null}
        onOpenChange={(o) => {
          if (!o) setRevoking(null);
        }}
        title="Revoke token"
        description="Agents stop posting as you and pending posts are cancelled."
        confirmLabel="Revoke token"
        danger
        busy={revoke.isPending}
        onConfirm={() => {
          if (!revoking) return;
          revoke.mutate(revoking.id, { onSettled: () => setRevoking(null) });
        }}
      />
    </>
  );
}
