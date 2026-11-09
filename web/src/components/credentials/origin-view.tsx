import type { components } from "@/lib/api/schema";

type Origin = components["schemas"]["StoredOrigin"];

const METHOD_LABEL: Record<string, string> = {
  litellm_team: "LiteLLM team",
  provider_account: "provider account",
};

const STATUS_LABEL: Record<string, string> = {
  mismatch: "The key belongs to a different account",
  unverifiable: "The account could not be verified",
  pending: "Verification pending",
};

/** `origin.detail` (C-502) in words. Unknown values are shown as sent (they are server-written, never key material). */
export function originDetailText(detail: string | null | undefined): string | null {
  if (!detail) return null;
  if (detail === "key_reports_no_team") return "This key belongs to no LiteLLM team";
  const http = /^key_info_http_(\d{3})$/.exec(detail);
  if (http) return `LiteLLM refused the key-info call (${http[1]})`;
  if (detail === "endpoint_unreachable") return "The endpoint could not be reached";
  if (detail === "bad_response") return "The endpoint returned a response Aurora could not read";
  return detail;
}

/** The account an LLM key was verified against: the method and the ids, or "Verified (endpoint)"; a failure says why. */
export function OriginView({ origin }: { origin: Origin | null | undefined }) {
  if (!origin) return <p className="text-xs text-muted">Account not verified.</p>;
  const verified = origin.status === "verified";
  const detail = originDetailText(origin.detail);
  const ids = [
    ["Organization", origin.organization_id],
    ["Workspace", origin.workspace_id],
    ["Team", origin.team_id],
  ].filter(([, v]) => !!v);
  // A key that reports no team verifies with `team_id = null` under `litellm_team` (C-502).
  const teamless = verified && origin.method === "litellm_team" && !origin.team_id;
  return (
    <dl className="text-xs text-ink-2">
      {verified && origin.method === "endpoint" ? (
        <div className="flex gap-1">
          <dt className="sr-only">Account</dt>
          <dd className="font-medium text-ink">Verified (endpoint)</dd>
        </div>
      ) : verified ? (
        <div className="flex gap-1">
          <dt>Verified account:</dt>
          <dd className="font-medium text-ink">{METHOD_LABEL[origin.method ?? ""] ?? origin.method ?? "verified"}</dd>
        </div>
      ) : (
        <div className="flex gap-1">
          <dt>Account:</dt>
          <dd className="font-medium text-danger-ink">{STATUS_LABEL[origin.status] ?? origin.status}</dd>
        </div>
      )}
      {detail ? (
        <div className="flex gap-1">
          <dt>Reason:</dt>
          <dd className="break-words">{detail}</dd>
        </div>
      ) : null}
      {origin.method === "endpoint" && verified
        ? null
        : ids.map(([k, v]) => (
            <div key={k as string} className="flex gap-1">
              <dt>{k}:</dt>
              <dd className="break-all font-mono">{v}</dd>
            </div>
          ))}
      {teamless ? (
        <div className="flex gap-1">
          <dt>Team:</dt>
          <dd>none (the key reports no team)</dd>
        </div>
      ) : null}
    </dl>
  );
}
