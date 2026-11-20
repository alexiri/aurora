import { accountRoutes } from "@/lib/routes";

/**
 * (P5-REFINE-2, org-3) "Go and come back" links carry where they came from as `?from=<path>`. Only a same-origin path
 * under `/o/` is followed: it must start with one `/` (not `//`), have no scheme, host, backslash or control character.
 * Anything else is dropped.
 */
export function safeFrom(raw: string | null | undefined): string | null {
  if (!raw || !raw.startsWith("/o/")) return null;
  if (raw.startsWith("//") || raw.includes("\\") || raw.includes("://")) return null;
  // eslint-disable-next-line no-control-regex -- control characters are the point
  if (/[\u0000-\u001f\u007f]/.test(raw)) return null;
  // No dot segments: `/o/../..//evil.test` must not be able to walk out of `/o/`.
  if (/(^|\/)\.\.?(\/|$)/.test(raw.split(/[?#]/)[0]!)) return null;
  try {
    const u = new URL(raw, "http://same-origin.invalid");
    if (u.origin !== "http://same-origin.invalid") return null;
  } catch {
    return null;
  }
  return raw;
}

/** The account's SSH keys page, remembering the page to return to. */
export function sshKeysHref(from: string): string {
  return `${accountRoutes.ssh}?from=${encodeURIComponent(from)}`;
}

/** `/o/<org>/w/<ws>/agents/<handle>…` → its parts, or `null`. */
export function agentPathParts(path: string): { org: string; ws: string; handle: string } | null {
  const m = /^\/o\/([^/?#]+)\/w\/([^/?#]+)\/agents\/([^/?#]+)/.exec(path);
  return m ? { org: decodeURIComponent(m[1]!), ws: decodeURIComponent(m[2]!), handle: decodeURIComponent(m[3]!) } : null;
}
