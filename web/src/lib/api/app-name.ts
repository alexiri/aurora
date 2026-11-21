// How an app is named everywhere (FIX-135, D-242, C-744): the product's family name first and the
// deployment second ("GitHub · Enterprise Server", "GitHub · github.com"); an app that is its own
// product is named as it is. Every list, picker, dialog and approval uses these, never `app.name`.

export interface NamedApp {
  name: string;
  family?: { id: string; name: string; variant_label: string; instance_bound?: boolean } | null;
}

/** "GitHub · Enterprise Server", or the app's own name without a family. */
export function appDisplayName(app: NamedApp): string {
  return app.family ? `${app.family.name} · ${app.family.variant_label}` : app.name;
}

/** The catalogue's grouping key: one product per family, each app on its own otherwise. */
export function productKey(app: NamedApp & { app_id: string }): string {
  return app.family ? `family:${app.family.id}` : `app:${app.app_id}`;
}

/** The product's name: the family's, or the app's own. */
export function productName(app: NamedApp): string {
  return app.family ? app.family.name : app.name;
}

// Which variants of a product can be connected right now (FIX-202, 14 §3 step 1). One rule for the App step, the Apps tab
// and the Sign in step, so they never disagree.

export interface ChoiceApp extends NamedApp {
  app_id: string;
  available?: boolean;
  readiness?: { state: string } | null;
}
export interface ChoiceInstance {
  id: string;
  app_id: string;
  status: string;
  builtin?: boolean;
}
export interface UsableChoice {
  appId: string;
  /** The registered server, for an instance-bound variant. */
  instanceId?: string;
}

/** The registered servers that count: active and not built in. */
export const registeredServers = <I extends ChoiceInstance>(instances: readonly I[] | undefined): I[] =>
  (instances ?? []).filter((i) => i.status === "active" && !i.builtin);

/**
 * The ways to connect a product: the hosted variant when it does not need an admin, then every registered server of a
 * usable instance-bound variant. An instance-bound variant with no active registration is not usable.
 */
export function usableChoices(members: readonly ChoiceApp[], instances: readonly ChoiceInstance[] | undefined): UsableChoice[] {
  const servers = registeredServers(instances);
  const out: UsableChoice[] = [];
  const open = members.filter((m) => m.available !== false && m.readiness?.state !== "needs_admin");
  for (const m of open.filter((x) => !x.family?.instance_bound)) out.push({ appId: m.app_id });
  for (const m of open.filter((x) => x.family?.instance_bound))
    for (const s of servers.filter((i) => i.app_id === m.app_id)) out.push({ appId: m.app_id, instanceId: s.id });
  return out;
}

/** A product is ready when at least one variant is usable; otherwise it needs an admin. */
export const isProductReady = (members: readonly ChoiceApp[], instances: readonly ChoiceInstance[] | undefined) =>
  usableChoices(members, instances).length > 0;
