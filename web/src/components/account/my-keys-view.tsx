"use client";

import { ErrorState, Loading } from "@/components/states";
import { Section } from "@/components/ui/section";
import { accountKeys, useMyApiKeys, useRevokeApiKey } from "@/lib/api/account";

import { KeyTable } from "./key-table";

export function MyKeysView() {
  const keys = useMyApiKeys();
  const revoke = useRevokeApiKey(accountKeys.myKeys);
  if (keys.isPending) return <Loading />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;
  return (
    <Section id="my-keys" title="API keys" description="Personal keys you created for workspaces (for example via the CLI).">
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
