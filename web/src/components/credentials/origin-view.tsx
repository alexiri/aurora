import type { components } from "@/lib/api/schema";

type Origin = components["schemas"]["StoredOrigin"];

export function OriginView({ origin }: { origin: Origin | null | undefined }) {
  if (!origin) return <p className="text-xs text-neutral-500">Account not verified.</p>;
  const ids = [
    ["Organization", origin.organization_id],
    ["Workspace", origin.workspace_id],
    ["Team", origin.team_id],
  ].filter(([, v]) => !!v);
  return (
    <dl className="text-xs text-neutral-700">
      <div className="flex gap-1">
        <dt>Verified account:</dt>
        <dd className="font-medium">{origin.status}</dd>
      </div>
      {ids.map(([k, v]) => (
        <div key={k as string} className="flex gap-1">
          <dt>{k}:</dt>
          <dd className="font-mono">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
