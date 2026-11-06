"use client";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import type { ContainerConfig } from "@/lib/api/agents";

export interface Revealed {
  token: string;
  config: ContainerConfig | null;
  agentName: string;
}

/** One-time token + container config reveal. State lives in the parent only. */
export function AgentRevealDialog({
  revealed,
  onClose,
}: {
  revealed: Revealed | null;
  onClose: () => void;
}) {
  return (
    <OneTimeKeyDialog
      secret={revealed?.token ?? null}
      onClose={onClose}
      title="Copy the agent token"
      description={`Agent "${revealed?.agentName ?? ""}" token. You won't see it again, so store it somewhere safe now.`}
    >
      {revealed?.config ? (
        <div className="space-y-1">
          <p className="text-sm font-medium">Container config</p>
          <pre
            data-testid="revealed-config"
            className="max-h-48 overflow-auto rounded-md bg-neutral-100 p-3 text-xs"
          >
            {JSON.stringify(
              { proxy_url: revealed.config.proxy_url, env: revealed.config.env, ca_path: revealed.config.ca_path },
              null,
              2,
            )}
          </pre>
        </div>
      ) : null}
    </OneTimeKeyDialog>
  );
}
