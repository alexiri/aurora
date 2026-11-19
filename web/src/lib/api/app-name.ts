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
