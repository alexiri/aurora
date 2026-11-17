"use client";

import { useEffect, useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { WhoCan } from "@/components/ui/who-can";
import { productKey, productName } from "@/lib/api/app-name";
import {
  useAppInstances,
  useApps,
  useConnectPlan,
  type AppSummary,
  type ConnectPlan,
} from "@/lib/api/apps";
import { cn } from "@/lib/utils";

import { DeploymentStep } from "../connect-dialog";

export type AppChoice = { kind: "app"; appId: string; instanceId?: string } | { kind: "website" };

/** A plan that no way to connect works for: nobody here can sign in yet (C-955). */
export function planBlocked(plan: ConnectPlan): boolean {
  return !plan.available || !plan.methods.some((m) => m.ready && m.audience !== "admin");
}

/** Reads one tile's plan once the tile is focused or clicked, never all at once (C-914). */
function PlanProbe({ app, workspaceId, onPlan }: { app: string; workspaceId: string; onPlan: (app: string, plan: ConnectPlan) => void }) {
  const plan = useConnectPlan(app, workspaceId);
  useEffect(() => {
    if (plan.data) onPlan(app, plan.data);
  }, [plan.data, app, onPlan]);
  return null;
}

function Tile({
  name,
  appId,
  hasDarkIcon,
  hint,
  blocked,
  onFocus,
  onClick,
}: {
  name: string;
  appId: string;
  hasDarkIcon?: boolean;
  hint?: string;
  blocked?: boolean;
  onFocus?: () => void;
  onClick: () => void;
}) {
  return (
    <li className="min-w-0">
      <button
        type="button"
        aria-disabled={blocked ? true : undefined}
        aria-label={blocked ? `${name} (needs setup)` : undefined}
        onFocus={onFocus}
        onMouseEnter={onFocus}
        onClick={() => {
          onFocus?.();
          if (!blocked) onClick();
        }}
        className={cn(
          "flex min-h-24 w-full flex-col items-center gap-1.5 rounded-control border border-line bg-surface p-3 text-center text-xs font-semibold text-ink hover:border-line-strong focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent",
          blocked && "cursor-not-allowed bg-canvas text-muted hover:border-line",
        )}
      >
        <AppIcon appId={appId} name={name} hasDarkIcon={hasDarkIcon ?? false} size={32} />
        <span className="break-words">{name}</span>
        {hint ? <span className="text-xs font-normal text-muted">{hint}</span> : null}
      </button>
    </li>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section aria-label={title} className="space-y-2">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-muted">{title}</h3>
      {children}
    </section>
  );
}

/**
 * **App** (`14 §3` step 1): the workspace's apps as tiles grouped Ready to connect, Already connected and Needs setup
 * by an admin, and **Another website or API** last. Apps the organization does not allow are not shown. A blocked tile
 * says who can fix it and cannot be opened. A family is one tile (FIX-135): the hosted service is assumed, and "Where
 * does your <App> run?" is asked only when a self-hosted server is registered.
 */
export function AppStep({ workspaceId, orgId, onPick }: { workspaceId: string; orgId: string; onPick: (c: AppChoice) => void }) {
  const apps = useApps(workspaceId);
  const instances = useAppInstances({ kind: "organization", id: orgId });
  const [plans, setPlans] = useState<Record<string, ConnectPlan>>({});
  const [probing, setProbing] = useState<string[]>([]);
  const [where, setWhere] = useState<AppSummary[] | null>(null);
  const onPlan = (app: string, plan: ConnectPlan) => setPlans((p) => (p[app] === plan ? p : { ...p, [app]: plan }));
  const probe = (app: string) => setProbing((p) => (p.includes(app) ? p : [...p, app]));

  if (apps.isPending) return <Loading />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => void apps.refetch()} />;
  const items = apps.data.items.filter((a) => a.available);
  const grouped = new Map<string, AppSummary[]>();
  for (const a of items) grouped.set(productKey(a), [...(grouped.get(productKey(a)) ?? []), a]);
  const products = [...grouped.values()].map((m) => [...m].sort((x, y) => Number(!!x.family?.instance_bound) - Number(!!y.family?.instance_bound)));

  if (where) {
    const family = where[0]!.family!;
    return <DeploymentStep family={family} siblings={where} orgId={orgId} onPick={(w) => onPick({ kind: "app", ...w })} />;
  }

  const servers = (instances.data?.items ?? []).filter((i) => i.status === "active" && !i.builtin);
  function choose(members: AppSummary[]) {
    const first = members[0]!;
    const bound = members.filter((m) => m.family?.instance_bound);
    if (first.family && members.length > 1 && bound.some((b) => servers.some((s) => s.app_id === b.app_id))) return setWhere(members);
    const hosted = members.find((m) => !m.family?.instance_bound) ?? first;
    onPick({ kind: "app", appId: hosted.app_id });
  }

  const planOf = (members: AppSummary[]) => plans[(members.find((m) => !m.family?.instance_bound) ?? members[0]!).app_id];
  const blockedOf = (members: AppSummary[]) => {
    const p = planOf(members);
    return p && planBlocked(p) ? p : null;
  };
  const rest = (members: AppSummary[]) => !blockedOf(members);
  const ready = products.filter((m) => rest(m) && m.every((x) => x.connection_count === 0));
  const connected = products.filter((m) => rest(m) && m.some((x) => x.connection_count > 0));
  const blocked = products.filter((m) => !rest(m));

  const tile = (members: AppSummary[], hint?: string) => {
    const first = members.find((m) => !m.family?.instance_bound) ?? members[0]!;
    return (
      <Tile
        key={productKey(first)}
        name={productName(first)}
        appId={first.app_id}
        hasDarkIcon={first.has_dark_icon}
        hint={hint}
        blocked={!rest(members)}
        onFocus={() => probe(first.app_id)}
        onClick={() => choose(members)}
      />
    );
  };

  return (
    <div className="space-y-4">
      {probing.map((id) => (
        <PlanProbe key={id} app={id} workspaceId={workspaceId} onPlan={onPlan} />
      ))}
      <Group title="Ready to connect">
        <ul className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {ready.map((m) => tile(m))}
          <li className="min-w-0">
            <button
              type="button"
              onClick={() => onPick({ kind: "website" })}
              className="flex min-h-24 w-full flex-col items-center justify-center gap-1.5 rounded-control border border-dashed border-line-strong bg-surface p-3 text-center text-xs font-semibold text-ink hover:border-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
            >
              <span aria-hidden="true" className="text-lg leading-none">+</span>
              Another website or API
            </button>
          </li>
        </ul>
      </Group>
      {connected.length > 0 ? (
        <Group title="Already connected">
          <ul className="grid grid-cols-2 gap-2 sm:grid-cols-3">{connected.map((m) => tile(m, "Connected"))}</ul>
        </Group>
      ) : null}
      {blocked.length > 0 ? (
        <Group title="Needs setup by an admin">
          <ul className="space-y-2">
            {blocked.map((m) => {
              const first = m.find((x) => !x.family?.instance_bound) ?? m[0]!;
              const plan = blockedOf(m)!;
              const who = plan.available_who_can ?? plan.methods.find((x) => x.who_can)?.who_can ?? null;
              return (
                <li key={productKey(first)} className="flex items-start gap-3 rounded-control border border-line bg-canvas p-3">
                  <AppIcon appId={first.app_id} name={productName(first)} hasDarkIcon={first.has_dark_icon} size={32} />
                  <div className="min-w-0 text-sm">
                    <p className="font-semibold text-muted">{productName(first)}</p>
                    <p className="text-ink-2">Sign-in is not set up.</p>
                    {who ? <WhoCan value={who} /> : null}
                  </div>
                </li>
              );
            })}
          </ul>
        </Group>
      ) : null}
    </div>
  );
}
