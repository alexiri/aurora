"use client";

import Link from "next/link";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Button, buttonVariants } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { isProductReady, productKey, productName, usableChoices } from "@/lib/api/app-name";
import { useAskSetup, useWithdrawSetup } from "@/lib/api/app-setup-requests";
import { useAppInstances, useApps, type AppSummary } from "@/lib/api/apps";
import { cn } from "@/lib/utils";

import { DeploymentStep } from "../connect-dialog";
import { StepHeader } from "./frame";
import { needsReason, signInProvider } from "./logic";

export type AppChoice = { kind: "app"; appId: string; instanceId?: string } | { kind: "website" };

/** The apps of a product (a family is one product), the hosted one first. */
type Product = AppSummary[];
const hostedOf = (p: Product) => p.find((m) => !m.family?.instance_bound) ?? p[0]!;

function Group({ title, count, children }: { title: string; count?: number; children: React.ReactNode }) {
  return (
    <section aria-label={count !== undefined ? `${title} · ${count}` : title} className="space-y-2">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-muted">
        {title}
        {count !== undefined ? ` · ${count}` : ""}
      </h3>
      {children}
    </section>
  );
}

function Tile({ app, onClick }: { app: AppSummary; onClick: () => void }) {
  return (
    <li className="min-w-0">
      <button
        type="button"
        onClick={onClick}
        className="flex min-h-24 w-full flex-col items-center gap-1.5 rounded-control border border-line bg-surface p-3 text-center text-xs font-semibold text-ink hover:border-line-strong focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
      >
        <AppIcon appId={app.app_id} name={productName(app)} hasDarkIcon={app.has_dark_icon} />
        <span className="break-words">{productName(app)}</span>
      </button>
    </li>
  );
}

/** A list row: icon, two lines and the action on the right (it wraps under on a phone). */
function Row({ app, title, line, children }: { app: AppSummary; title: string; line: React.ReactNode; children: React.ReactNode }) {
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-control border border-line bg-surface p-3">
      <AppIcon appId={app.app_id} name={title} hasDarkIcon={app.has_dark_icon} />
      <div className="min-w-0 flex-1 basis-40 text-sm">
        <p className="font-semibold text-ink">{title}</p>
        <p className="text-ink-2">{line}</p>
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </li>
  );
}

/**
 * **App** (`14 §3` step 1, P5-POLISH-5, D-388, B1/B2): the workspace's apps from one list (`readiness`, `accounts`,
 * C-1029), with a search field. **Ready to connect** (alphabetical, **Another website or API** last), **Already
 * connected** (an account this agent cannot use yet: **Give access**) and **Needs an admin first** (the reason in one
 * line and **Ask <Name>**, which turns into the asked state with **Undo**). Apps the organization switched off are not
 * listed. A family is one tile (FIX-135): the hosted service is assumed, and "Where does your <App> run?" is asked only
 * when a self-hosted server is registered.
 */
export function AppStep({
  workspaceId,
  orgId,
  agent,
  onPick,
  onGive,
  initialWhere,
}: {
  workspaceId: string;
  orgId: string;
  /** (FIX-202) Open on "Where does your <App> run?" for this family's apps (the Apps tab, "Use <variant>"). */
  initialWhere?: AppSummary[] | null;
  /** The agent the app is for (a fixed agent): "connected, <Agent> can't use it yet". */
  agent?: { id: string; name: string };
  onPick: (c: AppChoice) => void;
  /** Give an existing account to the agent: the Access step with that account chosen. */
  onGive?: (appId: string, accountId: string) => void;
}) {
  const apps = useApps(workspaceId);
  const instances = useAppInstances({ kind: "organization", id: orgId });
  const ask = useAskSetup(workspaceId);
  const withdraw = useWithdrawSetup(workspaceId);
  const [where, setWhere] = useState<AppSummary[] | null>(initialWhere ?? null);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<"ready" | "needs">("ready");
  // What the person did here, until the list is read again (an ask or an Undo changes `requested.by_me`).
  const [asked, setAsked] = useState<Record<string, "now" | "no">>({});

  if (apps.isPending || (instances.isPending && !where)) return <Loading />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => void apps.refetch()} />;
  const grouped = new Map<string, Product>();
  for (const a of apps.data.items.filter((x) => x.available)) grouped.set(productKey(a), [...(grouped.get(productKey(a)) ?? []), a]);
  const products: Product[] = [...grouped.values()]
    .map((m) => [...m].sort((x, y) => Number(!!x.family?.instance_bound) - Number(!!y.family?.instance_bound)))
    .sort((a, b) => productName(hostedOf(a)).localeCompare(productName(hostedOf(b))));

  if (where) {
    const family = where[0]!.family!;
    return <DeploymentStep family={family} siblings={where} orgId={orgId} onPick={(w) => onPick({ kind: "app", ...w })} />;
  }

  const registered = instances.data?.items;
  // (FIX-202) A family is ready when any variant is usable: one choice opens directly, several ask where it runs.
  function choose(members: Product) {
    const options = usableChoices(members, registered);
    if (options.length > 1) return setWhere(members);
    const only = options[0];
    onPick({ kind: "app", appId: only?.appId ?? hostedOf(members).app_id, ...(only?.instanceId ? { instanceId: only.instanceId } : {}) });
  }

  const q = query.trim().toLowerCase();
  const matches = (p: Product) => !q || p.some((a) => `${productName(a)} ${a.summary ?? ""} ${a.description}`.toLowerCase().includes(q));
  // An account of the product the agent cannot use yet (with no fixed agent, any account counts).
  const givable = (p: Product) => p.flatMap((m) => (m.accounts ?? []).map((a) => ({ app: m, account: a }))).filter((x) => !agent || !x.account.agent_ids.includes(agent.id));
  const ready: Product[] = [];
  const connected: Product[] = [];
  const blocked: Product[] = [];
  for (const p of products.filter(matches)) {
    if (givable(p).length > 0) connected.push(p);
    else if (!isProductReady(p, registered)) blocked.push(p);
    else ready.push(p);
  }
  const showWebsite = !q || "another website or api".includes(q);
  const agentName = agent?.name ?? "the agent";
  const seenProviders = new Set<string>();

  return (
    <div className="space-y-4">
      {/* (FIX-210) The search and the two tabs live in the dialog's fixed header, under the stepper: only the list scrolls, and nothing is sticky. (P5-REFINE-13, access-7) The counts come from the catalogue; a search narrows both. */}
      <StepHeader>
        <input
          type="search"
          aria-label="Search apps"
          placeholder="Search apps"
          autoComplete="off"
          className={inputClass}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Tabs
          label="Apps"
          idBase="app-step"
          value={tab}
          onValueChange={(v) => setTab(v as "ready" | "needs")}
          items={[
            { value: "ready", label: `Ready · ${ready.length}` },
            { value: "needs", label: `Needs an admin · ${blocked.length}` },
          ]}
        />
      </StepHeader>
      {/* (P5-POLISH-5, B1) While searching, a group with no match is hidden; "No app matches" only when every group is empty. */}
      {q && ready.length === 0 && !showWebsite && connected.length === 0 && blocked.length === 0 ? (
        <p className="text-sm text-ink-2">No app matches “{query.trim()}”.</p>
      ) : null}
      {tab === "ready" ? (
        <div role="tabpanel" id={tabPanelId("app-step", "ready")} aria-labelledby={tabId("app-step", "ready")} className="space-y-4">
      {!q || ready.length > 0 || showWebsite ? (
      <Group title="Ready to connect" count={ready.length}>
        <ul className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {ready.map((p) => (
            <Tile key={productKey(hostedOf(p))} app={hostedOf(p)} onClick={() => choose(p)} />
          ))}
          {showWebsite ? (
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
          ) : null}
        </ul>
      </Group>
      ) : null}
      {connected.length > 0 ? (
        <Group title="Already connected" count={connected.length}>
          <ul className="space-y-2">
            {connected.map((p) => {
              const g = givable(p)[0]!;
              return (
                <Row
                  key={productKey(g.app)}
                  app={hostedOf(p)}
                  title={g.account.name}
                  line={`${productName(g.app)} · connected, ${agentName} can't use it yet`}
                >
                  <Button size="sm" variant="outline" onClick={() => onGive?.(g.app.app_id, g.account.id)}>
                    Give access
                  </Button>
                </Row>
              );
            })}
          </ul>
        </Group>
      ) : null}
      {blocked.length > 0 ? (
        <p className="text-sm text-ink-2">
          {blocked.length} more need an admin to set them up first: {blocked.slice(0, 5).map((p) => productName(hostedOf(p))).join(", ")}
          {blocked.length > 5 ? "…" : ""}{" "}
          <button type="button" className="font-medium text-link underline" onClick={() => setTab("needs")}>
            See them
          </button>
        </p>
      ) : null}
        </div>
      ) : (
        <div role="tabpanel" id={tabPanelId("app-step", "needs")} aria-labelledby={tabId("app-step", "needs")} className="space-y-4">
      {blocked.length === 0 ? <p className="text-sm text-ink-2">{q ? "No app that needs an admin matches." : "Nothing here needs an admin."}</p> : null}
      {blocked.length > 0 ? (
        <Group title="Needs an admin first" count={blocked.length}>
          <ul className="space-y-2">
            {blocked.map((p) => {
              const app = hostedOf(p);
              const r = app.readiness!;
              const who = r.who_can ?? null;
              const provider = signInProvider(app);
              const shared = r.needs?.[0] === "oauth_client" && seenProviders.has(provider);
              if (r.needs?.[0] === "oauth_client") seenProviders.add(provider);
              const mine = asked[app.app_id] ? asked[app.app_id] === "now" : !!r.requested?.by_me;
              const busy = ask.isPending || withdraw.isPending;
              return (
                <Row
                  key={productKey(app)}
                  app={app}
                  title={productName(app)}
                  line={
                    mine ? (
                      <span className="flex items-start gap-2">
                        <span aria-hidden="true" className="mt-1.5 size-2 shrink-0 rounded-full bg-accent" />
                        <span>
                          Asked{asked[app.app_id] === "now" ? " just now" : ""}. You&apos;ll get a notice when it&apos;s ready, and {agentName}&apos;s Apps &amp; access tab will offer it.
                        </span>
                      </span>
                    ) : (
                      needsReason(r.needs, provider, shared)
                    )
                  }
                >
                  {mine ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => withdraw.mutate(app.app_id, { onSuccess: () => setAsked((s) => ({ ...s, [app.app_id]: "no" })) })}
                    >
                      Undo
                    </Button>
                  ) : who?.you_can && who.path ? (
                    <Link href={who.path} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                      Set up
                    </Link>
                  ) : (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      onClick={() => ask.mutate(app.app_id, { onSuccess: () => setAsked((s) => ({ ...s, [app.app_id]: "now" })) })}
                    >
                      Ask an admin
                    </Button>
                  )}
                </Row>
              );
            })}
          </ul>
        </Group>
      ) : null}
        </div>
      )}
    </div>
  );
}
