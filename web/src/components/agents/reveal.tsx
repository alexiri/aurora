"use client";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { expiryText } from "@/lib/api/agent-tokens";
import type { ContainerConfig } from "@/lib/api/agents";

export interface Revealed {
  token: string;
  config: ContainerConfig | null;
  agentName: string;
  /** (RFE-013) When the token expires; `null` / absent = never. */
  expiresAt?: string | null;
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
      {revealed ? (
        <p data-testid="revealed-expiry" className="text-sm text-ink-2">
          {revealed.expiresAt
            ? `This token expires on ${new Date(revealed.expiresAt).toLocaleDateString()} (${expiryText(revealed.expiresAt).toLowerCase()}). Regenerate it before then.`
            : "This token does not expire."}
        </p>
      ) : null}
      {revealed?.config ? (
        <div className="space-y-2">
          <p className="text-sm font-medium text-ink">Container config</p>
          <pre
            data-testid="revealed-config"
            className="max-h-48 overflow-auto rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink"
          >
            {JSON.stringify(
              {
                proxy_url: revealed.config.proxy_url,
                env: revealed.config.env,
                ca_path: revealed.config.ca_path,
                proxy_ca_path: revealed.config.proxy_ca_path,
              },
              null,
              2,
            )}
          </pre>
          <p data-testid="proxy-tls-note" className="text-xs text-muted">
            The gateway proxy is TLS-only (<code>https://</code>). Clients that treat the proxy leg separately need the CA
            for it too: curl <code>--proxy-cacert</code>, git <code>http.proxySSLCAInfo</code>; Node needs an HTTPS proxy
            agent (<code>undici</code> ProxyAgent), Python needs <code>urllib3</code> 2 or newer. A tool that cannot use an
            HTTPS proxy can run behind <code>aurora run --forward</code>.
          </p>
        </div>
      ) : null}
    </OneTimeKeyDialog>
  );
}
