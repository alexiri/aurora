"use client";

import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Section } from "@/components/ui/section";
import { accountKeys, useMyApiKeys, useRevokeApiKey } from "@/lib/api/account";

import { KeyTable } from "./key-table";

export function MyKeysView() {
  const keys = useMyApiKeys();
  const revoke = useRevokeApiKey(accountKeys.myKeys);
  if (keys.isPending) return <Loading />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;
  return (
    <Section id="my-keys" title="API keys">
      {/* (P5-REFINE-3) Set up: what the keys are for. The list itself stays for everyone who has keys. */}
      <Altitude id="apikeys" level="setup">
        <p className="text-sm text-ink-2">Personal keys you created for workspaces (for example via the CLI).</p>
      </Altitude>
      <KeyTable
        keys={keys.data.items}
        showWorkspace
        onRevoke={(id) => revoke.mutateAsync(id)}
        revoking={revoke.isPending}
        error={revoke.error}
      />
    </Section>
  );
}
