"use client";

// Open SSH sessions (P4-SSH-2, D-239, C-793, C-795, docs/14 §Account › SSH keys and §Agent page › SSH): one table for the
// person's own sessions on every agent (Account) and for the sessions on one agent (the agent's SSH page, where someone who
// can own the agent also sees the person). Disconnect asks the API to end a session (202); the terminator closes it at its
// next heartbeat, so the row stays until the list refreshes. Shared primitives only.
import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { useDisconnectSshSession, type SshSession } from "@/lib/api/ssh";
import { fmtDate } from "@/lib/utils";

/** The key a session used: its name, or the fingerprint once the key was deleted. */
function keyLabel(s: SshSession): string {
  return s.key.name ?? `${s.key.fingerprint} (deleted key)`;
}

export function SshSessionsList({
  query,
  scope,
  emptyWhen,
}: {
  query: { isPending: boolean; isError: boolean; error: unknown; data?: SshSession[]; refetch: () => unknown };
  /** `account`: the person's sessions on any agent (shows the agent); `agent`: one agent's (shows the person when the API sends it). */
  scope: "account" | "agent";
  emptyWhen: string;
}) {
  const disconnect = useDisconnectSshSession();
  const [target, setTarget] = useState<SshSession | null>(null);

  if (query.isPending) return <Loading />;
  if (query.isError) return <ErrorState error={query.error} onRetry={() => void query.refetch()} />;
  const sessions = query.data ?? [];
  if (sessions.length === 0) return <EmptyState title="No active SSH sessions" when={emptyWhen} />;

  const showUser = scope === "agent" && sessions.some((s) => s.user);
  const own = target?.user === undefined || target?.user === null;
  return (
    <>
      <ErrorNotice error={disconnect.error} />
      {disconnect.isSuccess ? (
        <Notice tone="neutral" role="status">
          Disconnecting. The session ends within 30 seconds.
        </Notice>
      ) : null}
      <DataTable caption="Active SSH sessions">
        <thead>
          <tr>
            {scope === "account" ? <Th>Agent</Th> : null}
            {showUser ? <Th>Person</Th> : null}
            <Th>Key</Th>
            <Th>From</Th>
            <Th>Opened</Th>
            <Th>Last activity</Th>
            <Th>Channels</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((s) => (
            <tr key={s.id}>
              {scope === "account" ? <Td className="break-words font-medium">{s.agent.name}</Td> : null}
              {showUser ? <Td className="break-words">{s.user?.label ?? "—"}</Td> : null}
              <Td className="break-words">{keyLabel(s)}</Td>
              <Td className="whitespace-nowrap text-ink-2">{s.client_address ?? "—"}</Td>
              <Td className="whitespace-nowrap text-ink-2">{fmtDate(s.opened_at)}</Td>
              <Td className="whitespace-nowrap text-ink-2">{fmtDate(s.last_activity_at)}</Td>
              <Td className="text-ink-2">{s.channels_total}</Td>
              <Td className="text-right">
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={`Disconnect the session on ${s.agent.name} opened ${fmtDate(s.opened_at)}`}
                  onClick={() => {
                    disconnect.reset();
                    setTarget(s);
                  }}
                >
                  Disconnect
                </Button>
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(o) => {
          if (!o) setTarget(null);
        }}
        title="Disconnect SSH session"
        description={
          target
            ? own
              ? `End your SSH session on ${target.agent.name}? Running commands in it stop.`
              : `End ${target.user?.label ?? "this person"}'s SSH session on ${target.agent.name}? Running commands in it stop.`
            : ""
        }
        confirmLabel="Disconnect"
        danger
        busy={disconnect.isPending}
        onConfirm={() => {
          if (!target) return;
          disconnect.mutate(target.id, { onSettled: () => setTarget(null) });
        }}
      />
    </>
  );
}
