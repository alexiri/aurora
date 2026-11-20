"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useState, type ReactNode } from "react";

import { AppIcon } from "@/components/ui/app-icon";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { inputClass } from "@/components/ui/field";
import { checkSummary, useAddonSources, useCheckAllAddonSources } from "@/lib/api/addon-sources";
import { errorDetail, errorMessage } from "@/lib/api/errors";
import { usePlugins, useUploadPlugin, type PluginPackage, type PluginViolation } from "@/lib/api/plugins";
import { adminRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

export function violationsOf(err: unknown): PluginViolation[] {
  const v = errorDetail(err, "violations");
  return Array.isArray(v) ? (v as PluginViolation[]) : [];
}

export function ViolationList({ violations }: { violations: PluginViolation[] }) {
  if (violations.length === 0) return null;
  return (
    <ul aria-label="Violations" className="space-y-1 text-xs text-danger-ink">
      {violations.map((v, i) => (
        <li key={`${v.code}-${v.path}-${i}`}>
          <code className="font-mono">{v.code}</code> {v.path ? <code className="font-mono">{v.path}</code> : null} {v.message}
        </li>
      ))}
    </ul>
  );
}

/** Tone for a plugin lifecycle status (text always shown beside it). */
export function statusTone(status: string): "neutral" | "accent" | "danger" {
  if (status === "active") return "accent";
  if (status === "rejected" || status === "invalid") return "danger";
  return "neutral";
}

export function UploadForm({ onDone }: { onDone?: () => void }) {
  const upload = useUploadPlugin();
  const [file, setFile] = useState<File | null>(null);
  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!file) return;
        upload.mutate(file, { onSuccess: () => onDone?.() });
      }}
    >
      <div className="space-y-1.5">
        <label className="block text-sm font-medium text-ink" htmlFor="plugin-package">
          Plugin package
        </label>
        <input
          id="plugin-package"
          type="file"
          className={`${inputClass} file:mr-3 file:rounded-control file:border-0 file:bg-subtle file:px-3 file:py-1 file:text-sm file:font-medium file:text-ink`}
          onChange={(e) => setFile(e.target.files?.[0] ?? null)}
        />
      </div>
      {upload.isError ? (
        <Notice tone="danger" role="alert" className="space-y-1">
          <p>{errorMessage(upload.error)}</p>
          <ViolationList violations={violationsOf(upload.error)} />
        </Notice>
      ) : null}
      {upload.data?.status === "invalid" ? (
        <Notice tone="danger" role="alert" className="space-y-1">
          <p>The package was stored but is invalid.</p>
          <ViolationList violations={upload.data.violations} />
        </Notice>
      ) : null}
      <div className="flex justify-end">
        <Button type="submit" size="lg" disabled={!file || upload.isPending}>
          {upload.isPending ? "Uploading…" : "Upload"}
        </Button>
      </div>
    </form>
  );
}

/** Where a package came from: the addon source's name, or the plain source kind. */
export function sourceLabel(p: { source: string; source_name?: string | null }): string {
  if (p.source === "addon") return p.source_name ?? "Addon source (removed)";
  return p.source;
}

/** (C-1032) Where a package came from, in the words the list shows. */
export function originLabel(p: Pick<PluginPackage, "origin" | "source" | "source_name">): string {
  switch (p.origin) {
    case "aurora":
      return "From Aurora";
    case "uploaded":
      return "Uploaded";
    case "directory":
      return "From the plugin directory";
    case "addon":
      return p.source_name ?? "Addon source";
    default:
      return p.origin;
  }
}

/** "reaches api.github.com", "reaches a.test, b.test and 2 more", or `null` when the package reaches no host. */
export function reachesLine(p: Pick<PluginPackage, "reaches" | "reaches_more">): string | null {
  if (p.reaches.length === 0) return null;
  const more = p.reaches_more > 0 ? ` and ${p.reaches_more} more` : "";
  return `${p.reaches.join(", ")}${more}`;
}

/** The app's name: the package's first app, else its id. */
export function packageName(p: Pick<PluginPackage, "apps" | "package_id">): string {
  return p.apps[0]?.name || p.package_id;
}

/** "check for updates": checks every addon source at once and reports each one's outcome. */
function CheckForUpdates() {
  const sources = useAddonSources();
  const checkAll = useCheckAllAddonSources();
  const items = sources.data?.items ?? [];
  if (items.length === 0) return null;
  const names = new Map(items.map((s) => [s.id, s.name]));
  return (
    <div className="space-y-2">
      <Button variant="outline" disabled={checkAll.isPending} onClick={() => checkAll.mutate()}>
        {checkAll.isPending ? "Checking…" : "Check for updates"}
      </Button>
      {checkAll.isError ? (
        <ErrorNotice error={checkAll.error} />
      ) : null}
      {checkAll.data ? (
        <Notice tone="neutral" role="status" aria-label="Update check results">
          <ul className="space-y-0.5">
            {checkAll.data.items.map((r) => (
              <li key={r.source_id}>{checkSummary(r, names.get(r.source_id) ?? "Addon source")}</li>
            ))}
          </ul>
        </Notice>
      ) : null}
    </div>
  );
}

/** One app row: icon, name, one line, and one action. */
function AppRow({ p, action, line, badge }: { p: PluginPackage; action: string; line: string; badge?: ReactNode }) {
  const app = p.apps[0];
  const name = packageName(p);
  return (
    <li className="flex items-center gap-3 px-4 py-3 sm:px-5">
      <AppIcon appId={app?.app_id ?? p.package_id} name={name} hasIcon={Boolean(app)} size={32} />
      <div className="min-w-0 flex-1">
        <Link href={adminRoutes.package(p.id)} className="block truncate font-medium text-ink hover:underline">
          {name}
        </Link>
        <p className="truncate text-xs text-muted">{line}</p>
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
        {badge}
        {p.update_available ? (
          <AttentionChip
            action={
              <Link className="text-xs font-medium text-link underline" href={adminRoutes.advanced(p.source_id ?? "package-sources")}>
                Review the update
              </Link>
            }>
            Update available
          </AttentionChip>
        ) : null}
        <Link href={adminRoutes.package(p.id)} className={cn(buttonVariants({ variant: action === "Review" ? "primary" : "ghost", size: "sm" }), action === "Review" && "text-on-primary!")} aria-label={`${action} ${name}`}>
          {action}
        </Link>
      </div>
    </li>
  );
}

function waitingLine(p: PluginPackage): string {
  const reach = reachesLine(p);
  return [originLabel(p), p.signer.kind === "unsigned" ? "unsigned" : "signed", reach ? `reaches ${reach}` : null].filter(Boolean).join(" · ");
}

function auroraLine(p: PluginPackage): string {
  const reach = reachesLine(p);
  return [reach ? `Reaches ${reach}` : null, p.summary || null].filter(Boolean).join(" · ") || "No outside host";
}

function otherLine(p: PluginPackage): string {
  const reach = reachesLine(p);
  return [originLabel(p), reach ? `reaches ${reach}` : null].filter(Boolean).join(" · ");
}

const FIRST_AURORA = 3;

export function PluginsView() {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const approved = useSearchParams().get("approved");
  const q = usePlugins("");
  const [search, setSearch] = useState("");
  const all = q.data?.items ?? [];
  // (P5-REFINE-18 #12, admin-11) A to Z in every list, and one search over all of them.
  const term = search.trim().toLowerCase();
  const items = all
    .filter((p) => !term || packageName(p).toLowerCase().includes(term) || p.package_id.toLowerCase().includes(term))
    .sort((a, b) => packageName(a).localeCompare(packageName(b), undefined, { sensitivity: "base" }));
  const waiting = items.filter((p) => p.status === "pending_approval" && p.origin !== "aurora");
  const fromAurora = items.filter((p) => p.origin === "aurora" && p.status !== "superseded");
  const others = items.filter((p) => !waiting.includes(p) && p.origin !== "aurora" && p.status !== "superseded");
  const appCount = all.filter((p) => p.status !== "superseded").length;
  const updates = items.filter((p) => p.update_available).length;
  const shown = showAll ? fromAurora : fromAurora.slice(0, FIRST_AURORA);
  return (
    <div className="max-w-page space-y-5">
      <PageHeader
        title="Apps"
        description="The apps and chat platforms people can add."
        className="mb-2"
        actions={
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild>
              <Button>Upload app</Button>
            </DialogTrigger>
            <DialogContent title="Upload app" description="Upload an app package for review.">
              <UploadForm onDone={() => setOpen(false)} />
            </DialogContent>
          </Dialog>
        }
      />
      {approved ? (
        <Notice tone="accent" role="status" data-testid="approved-toast">
          {`${approved} approved. People can add it now.`}
        </Notice>
      ) : null}
      {updates > 0 ? (
        <Notice tone="pending" role="status">
          {`${updates} ${updates === 1 ? "app has" : "apps have"} a newer version in an addon source. `}
          <Link href={adminRoutes.advanced("package-sources")} className="font-medium underline">
            See the addon sources
          </Link>
        </Notice>
      ) : null}
      <CheckForUpdates />
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && all.length === 0 ? <EmptyState title="No apps yet" /> : null}
      {all.length > 0 ? (
        <div className="max-w-md">
          <label htmlFor="apps-search" className="sr-only">
            Search apps
          </label>
          <input
            id="apps-search"
            type="search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder={`Search ${appCount} apps`}
            autoComplete="off"
            className={inputClass}
          />
        </div>
      ) : null}
      {q.data && all.length > 0 && items.length === 0 ? <p className="text-sm text-ink-2">{`No app matches "${search.trim()}".`}</p> : null}
      {q.data && items.length > 0 ? (
        <>
          <section aria-labelledby="apps-waiting" className="space-y-3">
            <h2 id="apps-waiting" className="font-display text-lg font-semibold text-ink">
              {waiting.length === 0 ? "No app is waiting." : waiting.length === 1 ? "1 app is waiting for your approval." : `${waiting.length} apps are waiting for your approval.`}
            </h2>
            {waiting.length > 0 ? (
              <Card className="p-0">
                <ul className="divide-y divide-line">
                  {waiting.map((p) => (
                    <AppRow key={p.id} p={p} action="Review" line={waitingLine(p)} />
                  ))}
                </ul>
              </Card>
            ) : null}
          </section>
          {fromAurora.length > 0 ? (
            <section aria-labelledby="apps-aurora" className="space-y-3">
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <h2 id="apps-aurora" className="font-display text-lg font-semibold text-ink">
                  From Aurora
                </h2>
                <p className="text-sm text-ink-2">{`${fromAurora.length} ${fromAurora.length === 1 ? "app" : "apps"}, approved with this release`}</p>
              </div>
              <Card className="p-0">
                <ul className="divide-y divide-line">
                  {shown.map((p) => (
                    <AppRow key={p.id} p={p} action="Details" line={auroraLine(p)} />
                  ))}
                  {fromAurora.length > FIRST_AURORA ? (
                    <li className="px-4 py-2 sm:px-5">
                      <button type="button" aria-expanded={showAll} className="text-sm text-ink-2 hover:text-ink hover:underline" onClick={() => setShowAll((v) => !v)}>
                        {showAll ? "Show fewer" : `${fromAurora.length - FIRST_AURORA} more`}
                      </button>
                    </li>
                  ) : null}
                </ul>
              </Card>
            </section>
          ) : null}
          {others.length > 0 ? (
            <section aria-labelledby="apps-others" className="space-y-3">
              <h2 id="apps-others" className="font-display text-lg font-semibold text-ink">
                Other apps
              </h2>
              <Card className="p-0">
                <ul className="divide-y divide-line">
                  {others.map((p) => (
                    <AppRow
                      key={p.id}
                      p={p}
                      action="Details"
                      line={otherLine(p)}
                      badge={<Badge tone={statusTone(p.status)}>{p.status.replace("_", " ")}</Badge>}
                    />
                  ))}
                </ul>
              </Card>
            </section>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
