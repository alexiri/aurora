"use client";

import { useState } from "react";

import { SecretOnce } from "@/components/ui/secret-once";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { useAdminConnectors } from "@/lib/api/connectors";

import { AddConnectorFlow } from "./add-connector";
import { ConnectorCard } from "./connector-card";
import { ConnectorDetail } from "./connector-detail";
import { HostTokens } from "./host-tokens";
import { SelfHostedProvision } from "./self-hosted-provision";

export { ManifestDiff, ManifestSummary, diffManifest, type Change } from "./manifest-views";

type View = { kind: "list" } | { kind: "add" } | { kind: "self-hosted" } | { kind: "detail"; id: string };

/** Admin → Connectors: the list, Add connector, the self-hosted path, and a managed connector's detail. */
export function ConnectorsAdminView() {
  const q = useAdminConnectors();
  const [view, setView] = useState<View>({ kind: "list" });
  const [secret, setSecret] = useState<{ id: string; token: string } | null>(null);
  const list = () => setView({ kind: "list" });

  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;

  const detail = view.kind === "detail" ? q.data.items.find((c) => c.id === view.id) : undefined;

  let body;
  if (view.kind === "detail" && detail) {
    body = <ConnectorDetail c={detail} onBack={list} />;
  } else if (view.kind === "add") {
    body = (
      <>
        <AddConnectorFlow onSelfHosted={() => setView({ kind: "self-hosted" })} onAdded={(id) => setView({ kind: "detail", id })} />
        <Button variant="outline" onClick={list}>
          Cancel
        </Button>
      </>
    );
  } else if (view.kind === "self-hosted") {
    body = <SelfHostedProvision onToken={(id, token) => { setSecret({ id, token }); list(); }} onCancel={list} />;
  } else {
    body = (
      <Section id="conn-list" title="Chat platforms">
        {q.data.items.length === 0 ? (
          <EmptyState title="No chat platforms">Add one to let organizations connect to it.</EmptyState>
        ) : (
          <ul className="space-y-4">
            {q.data.items.map((c) => (
              <ConnectorCard key={c.id} c={c} onManage={(id) => setView({ kind: "detail", id })} onToken={(id, token) => setSecret({ id, token })} />
            ))}
          </ul>
        )}
      </Section>
    );
  }
  const tokens = view.kind === "list" ? <HostTokens /> : null;

  return (
    <div className="max-w-page space-y-8">
      {view.kind === "detail" && detail ? null : (
        <PageHeader
          title="Chat platforms"
          description="The chat platforms that organizations can connect to, and the hosts that run them."
          className="mb-6"
          actions={
            view.kind === "list" || view.kind === "detail" ? (
              <Button onClick={() => setView({ kind: "add" })}>Add chat platform</Button>
            ) : undefined
          }
        />
      )}
      {body}
      {tokens}
      <SecretOnce
        secret={secret?.token ?? null}
        title="Copy the chat platform token"
        description={`Token for chat platform "${secret?.id ?? ""}". You won't see it again, so give it to the chat platform host now.`}
        onClose={() => setSecret(null)}
      />
    </div>
  );
}
