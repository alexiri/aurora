/**
 * Splits a full URL typed or pasted into the self-hosted app's Host field into the host and the
 * base path (D-231): `https://its.cern.ch/jira/?x=1#y` becomes `its.cern.ch` and `/jira`. The
 * scheme, query and fragment are dropped.
 *
 * Returns `null` when there is nothing to split (a bare host), and when the input also has a port or
 * userinfo: it is left as typed so the server's `instance_host_invalid` explains. The server stays
 * authoritative; this only saves a retype.
 */
export function splitInstanceUrl(raw: string): { host: string; path: string } | null {
  const t = raw.trim();
  const m = /^(?:[a-z][a-z0-9+.-]*:\/\/)?([^/?#]*)([^?#]*)/i.exec(t);
  if (!m) return null;
  const hadScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(t);
  const [, authority, rest] = m;
  if (!hadScheme && rest === "") return null;
  if (!authority || authority.includes("@") || authority.includes(":")) return null;
  const path = rest === "/" ? "" : rest.replace(/\/+$/, "");
  return { host: authority.toLowerCase(), path };
}
