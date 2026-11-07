"use client";

import { ErrorState, Loading } from "@/components/states";
import { accountKeys, useMyApiKeys, useRevokeApiKey } from "@/lib/api/account";

import { KeyTable } from "./key-table";

export function MyKeysView() {
  const keys = useMyApiKeys();
  const revoke = useRevokeApiKey(accountKeys.myKeys);
  if (keys.isPending) return <Loading />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;
  return (
    <div className="space-y-3">
      <p className="text-sm text-neutral-600">
        Personal keys you created for workspaces (for example via the CLI).
      </p>
      <KeyTable
        keys={keys.data.items}
        showWorkspace
        onRevoke={(id) => revoke.mutateAsync(id)}
        revoking={revoke.isPending}
        error={revoke.error}
      />
    </div>
  );
}
