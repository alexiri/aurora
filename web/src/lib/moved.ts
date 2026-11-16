/**
 * "Pages have new names" (C-987, P5-IA-1): old name → new name, one table. The notice lists the rows the viewer can
 * open, each with a link; **What moved** in the account menu replays it. Paths come from `routes.ts`.
 */
import { adminRoutes, orgRoutes, wsRoutes } from "./routes";

/** The key stored in `user_onboarding.notices_seen` once the person dismissed the notice. */
export const RENAMES_NOTICE = "renames-2026-11";

/** Where the viewer can go: what decides which rows apply and what they link to. */
export interface MovedContext {
  instanceAdmin: boolean;
  /** An organization where the viewer is owner or admin (slug). */
  adminOrg?: string;
  /** Any organization the viewer belongs to (slug). */
  anyOrg?: string;
  /** A workspace the viewer owns. */
  ownerWs?: { org: string; ws: string };
  /** Any workspace the viewer can open. */
  anyWs?: { org: string; ws: string };
}

export interface MovedRow {
  from: string;
  to: string;
  href: string;
}

export function movedRows(c: MovedContext): MovedRow[] {
  const rows: MovedRow[] = [];
  const add = (from: string, to: string, href: string | undefined) => {
    if (href) rows.push({ from, to, href });
  };
  const ownerWs = c.ownerWs && wsRoutes(c.ownerWs.org, c.ownerWs.ws);
  const anyWs = c.anyWs && wsRoutes(c.anyWs.org, c.anyWs.ws);
  const adminOrg = c.adminOrg ? orgRoutes(c.adminOrg) : undefined;
  const anyOrg = c.anyOrg ? orgRoutes(c.anyOrg) : undefined;
  // Accounts are for owners and admins; Internet access and AI models are readable by every member.
  add("Credentials", "Apps & accounts", ownerWs?.accounts() ?? adminOrg?.accounts());
  add("Policy", "Internet access", anyWs?.internetAccess() ?? anyOrg?.internetAccess());
  add("Governance › Model access", "AI models", anyOrg?.aiModels());
  add("Governance › Egress", "Internet access", anyOrg?.internetAccess());
  add("Governance › Apps", "Apps & accounts › Apps", adminOrg?.apps("apps"));
  add("Governance › BYO agents", "Agents › BYO", adminOrg?.agents("byo"));
  add("Members", "People", adminOrg?.people("members") ?? ownerWs?.people());
  add("Settings › Access", "People", ownerWs?.people());
  add("Channels", "Chat platforms", adminOrg?.chatPlatforms());
  add("Storage and Audit", "Data", adminOrg?.data());
  if (c.instanceAdmin) {
    add("All organizations", "Administration", adminRoutes.home());
    add("Plugins", "Administration › Apps & accounts", adminRoutes.apps("apps"));
    add("Connectors", "Chat platforms", adminRoutes.chatPlatforms());
    add("Identity", "People › Sign-in", adminRoutes.signIn());
    add("Placement", "People › New users", adminRoutes.people("new-users"));
  }
  return rows;
}

/** Window event that asks the shell to show the notice again (the account menu's **What moved**). */
export const MOVED_EVENT = "aurora:what-moved";
export function showWhatMoved(): void {
  window.dispatchEvent(new Event(MOVED_EVENT));
}
