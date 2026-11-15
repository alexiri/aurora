import { type NamedApp } from "@/lib/api/app-name";

/**
 * The one way an app is named in the dashboard (FIX-135, D-242): the family name first, the
 * deployment second and muted ("GitHub · Enterprise Server"), plus the server's own address when
 * a registered one is meant. Plain text output for an app that is its own product.
 */
export function AppName({ app, host }: { app: NamedApp; host?: string | null }) {
  if (!app.family) {
    return (
      <>
        {app.name}
        {host ? <span className="text-muted">{` (${host})`}</span> : null}
      </>
    );
  }
  return (
    <>
      {app.family.name}
      <span className="text-muted">{` · ${app.family.variant_label}`}</span>
      {host ? <span className="text-muted">{` (${host})`}</span> : null}
    </>
  );
}
