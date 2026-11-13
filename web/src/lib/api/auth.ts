import { api } from "./client";
import { unwrap } from "./unwrap";

// Phase 1 (RFE-004, D-46): all sign-in is OIDC (full navigation to
// `/v1/auth/oidc/{provider}/start`); there are no password/verify/reset calls.

export const approveCli = (body: { user_code: string; workspace_id: string }) =>
  unwrap(api.POST("/v1/auth/cli/approve", { body }));

/** (FIX-108) What the pending login is asking for: host name, start time, coarse address. */
export const getPendingCli = (code: string) =>
  unwrap(api.GET("/v1/auth/cli/pending", { params: { query: { code } } }));

/** `GET /v1/auth/oidc/{provider}/start` URL (a browser navigation, never a fetch). */
export function oidcStartUrl(
  provider: string,
  opts: { returnTo?: string; reauth?: boolean } = {},
): string {
  const q = new URLSearchParams();
  if (opts.returnTo) q.set("return_to", opts.returnTo);
  if (opts.reauth) q.set("reauth", "1");
  const qs = q.toString();
  return `/v1/auth/oidc/${encodeURIComponent(provider)}/start${qs ? `?${qs}` : ""}`;
}
