"use client";

import { useState } from "react";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { fmtDate } from "@/components/account/key-table";
import { Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { errorMessage } from "@/lib/api/errors";
import { useHostTokens, useIssueHostToken, useRevokeHostToken } from "@/lib/api/connectors";

/**
 * Admin → Connectors → Connector hosts (FIX-120, C-690..C-692): one token per connector host,
 * bound to the connector types it runs. The shared bootstrap token is listed too; from the first
 * bound token on it is refused.
 */
export function HostTokens() {
  const q = useHostTokens();
  const issue = useIssueHostToken();
  const revoke = useRevokeHostToken();
  const [name, setName] = useState("");
  const [types, setTypes] = useState("");
  const [secret, setSecret] = useState<{ name: string; token: string } | null>(null);

  if (q.isPending) return <Loading />;
  if (q.isError) {
    return (
      <Notice tone="danger" role="status">
        Connector host tokens could not be loaded.{" "}
        <Button variant="outline" onClick={() => void q.refetch()}>
          Reload tokens
        </Button>
      </Notice>
    );
  }

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const type_ids = types.split(/[\s,]+/).filter(Boolean);
    issue.mutate(
      { name: name.trim(), type_ids },
      {
        onSuccess: (r) => {
          setSecret({ name: r.name ?? name, token: r.token });
          setName("");
          setTypes("");
        },
      },
    );
  };

  return (
    <Section id="host-tokens" title="Connector hosts">
      <p className="mb-3 text-sm text-ink-2">
        Each connector host authenticates with its own token, bound to the connector types it runs: a token can only register,
        receive credentials for and report on those types.
      </p>
      <DataTable caption="Connector host tokens">
        <thead>
          <tr>
            <Th>Name</Th>
            <Th>Connector types</Th>
            <Th>Status</Th>
            <Th>Last used</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {q.data.items.map((t) => (
            <tr key={t.id}>
              <Td>{t.name ?? "Shared bootstrap token"}</Td>
              <Td className="font-mono text-xs">{t.type_ids ? t.type_ids.join(", ") : "any (not bound)"}</Td>
              <Td>
                <Badge tone={t.state === "active" ? "accent" : t.state === "refused" ? "pending" : "danger"}>{t.state}</Badge>
              </Td>
              <Td className="whitespace-nowrap text-ink-2">{fmtDate(t.last_used_at)}</Td>
              <Td>
                {t.type_ids && t.state === "active" ? (
                  <Button
                    variant="outline"
                    disabled={revoke.isPending}
                    onClick={() => {
                      if (window.confirm(`Revoke the token "${t.name}"? Its host stops working on its next call.`)) revoke.mutate(t.id);
                    }}
                  >
                    Revoke
                  </Button>
                ) : null}
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      {q.data.items.some((t) => t.state === "refused") ? (
        <Notice tone="pending" className="mt-3">
          The shared bootstrap token is refused because a host token exists. Hosts still using it keep their running connectors
          until they are restarted with their own token.
        </Notice>
      ) : null}
      <form onSubmit={submit} className="mt-4 grid max-w-xl gap-3">
        <Field id="host-token-name" label="Name" hint="For example the Deployment's name.">
          <input id="host-token-name" className={inputClass} value={name} onChange={(e) => setName(e.target.value)} required maxLength={80} />
        </Field>
        <Field id="host-token-types" label="Connector types" hint="Comma-separated type ids, for example mattermost.">
          <input id="host-token-types" className={inputClass} value={types} onChange={(e) => setTypes(e.target.value)} required />
        </Field>
        {issue.isError ? (
          <Notice tone="danger" role="alert">
            {errorMessage(issue.error)}
          </Notice>
        ) : null}
        <div>
          <Button type="submit" disabled={issue.isPending}>
            Issue token
          </Button>
        </div>
      </form>
      <OneTimeKeyDialog
        secret={secret?.token ?? null}
        title="Copy the connector host token"
        description={`Token "${secret?.name ?? ""}". You won't see it again: set it as the host's AURORA_CONNECTOR_HOST_TOKEN now.`}
        onClose={() => setSecret(null)}
      />
    </Section>
  );
}
