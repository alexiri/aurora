"use client";

import Link from "next/link";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ErrorNotice } from "@/components/ui/notice";
import {
  useUpdateTemplate,
  type AdminTemplateView,
} from "@/lib/api/agent-templates";
import { adminRoutes } from "@/lib/routes";
import { fmtDate } from "@/lib/utils";

import { problemWords, useAppName } from "./words";

/** The apps the template needs (C-1108): the catalogue name, and "Not installed here" when this instance lacks it. */
function NeedsApps({ t, nameOf }: { t: AdminTemplateView; nameOf: (id: string) => string }) {
  if (t.needs_apps.length === 0) return <span className="text-muted">—</span>;
  return (
    <ul aria-label={`Apps ${t.name} needs`} className="space-y-0.5 text-xs">
      {t.needs_apps.map((a) => (
        <li key={a.app_id} className="min-w-0 [overflow-wrap:anywhere]">
          {nameOf(a.app_id) + (!a.installed ? " · Not installed here" : "")}
        </li>
      ))}
    </ul>
  );
}

/** The skill libraries the template references (C-1108): the repository, and "Not added here" when it is not. */
function NeedsLibraries({ t }: { t: AdminTemplateView }) {
  if (t.needs_libraries.length === 0) return <span className="text-muted">—</span>;
  return (
    <ul aria-label={`Skill libraries ${t.name} needs`} className="space-y-0.5 text-xs">
      {t.needs_libraries.map((l) => (
        <li key={l.url} className="min-w-0 [overflow-wrap:anywhere]">
          <span className="font-mono text-xs">
            {`${l.url}${l.ref ? ` · ${l.ref}` : ""}${l.path ? ` · ${l.path}` : ""}${!l.added_here ? " · Not added here" : ""}`}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** What the stored versions say (C-1114): the review link with its count, waiting-for-package, removed upstream. */
export function TemplateChips({ t }: { t: AdminTemplateView }) {
  const waitingReview = t.versions.filter((v) => v.status === "pending_approval");
  const waitingPackage = t.versions.filter((v) => v.status === "awaiting_package");
  const since = waitingPackage.map((v) => v.created_at).sort()[0];
  return (
    <div className="space-y-1.5">
      {waitingReview.length > 0 ? (
        <p>
          <Link
            href={adminRoutes.templateReview(t.template_id, waitingReview[0].version)}
            className="text-link underline"
            aria-label={`${waitingReview.length} ${waitingReview.length === 1 ? "version" : "versions"} of ${t.name} waiting for review, review ${waitingReview.length === 1 ? "it" : "them"}`}
          >
            Waiting for review ({waitingReview.length})
          </Link>
        </p>
      ) : null}
      {waitingPackage.length > 0 ? (
        <p>
          <Badge>Waiting for its package{since ? ` since ${fmtDate(since)}` : ""}</Badge>
        </p>
      ) : null}
      {t.removed_upstream ? (
        <p>
          <Badge tone="danger">Removed from the repository</Badge>
        </p>
      ) : null}
      {t.head_state !== "ok" ? (
        <div className="space-y-0.5">
          <p className="text-xs text-danger-ink">The repository&apos;s newest version has problems</p>
          <ul aria-label={`Problems of ${t.name}`} className="space-y-0.5">
            {(Array.isArray(t.head_problems) ? (t.head_problems as { message?: unknown; path?: unknown; dir?: unknown }[]) : []).map((p, i) => (
              <li key={i} className="text-xs text-danger-ink [overflow-wrap:anywhere]">
                {problemWords(p)}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {waitingReview.length === 0 && waitingPackage.length === 0 && !t.removed_upstream && t.head_state === "ok" ? <span className="text-muted">—</span> : null}
    </div>
  );
}

/**
 * The templates list (C-1114): name and summary, its source, the offered version, the chips (Waiting for review with
 * its count linking to the review page, Waiting for its package since when, Removed from the repository), the head's
 * problems, the apps and libraries it needs, and **Withdraw** / **Offer again**. Below 900 px each row is a card.
 */
export function TemplatesTable({ rows, caption }: { rows: AdminTemplateView[]; caption: string }) {
  const update = useUpdateTemplate();
  const nameOf = useAppName();
  const [error, setError] = useState<unknown>(null);
  return (
    <div className="space-y-3">
      <ErrorNotice error={error} />
      <DataTable caption={caption} stackBelow={900}>
        <thead>
          <tr>
            <Th>Template</Th>
            <Th>Source</Th>
            <Th>Offered version</Th>
            <Th>Status</Th>
            <Th>Apps it needs</Th>
            <Th>Skill libraries it needs</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((t) => (
            <tr key={t.template_id}>
              <Td cell="title" className="min-w-0">
                <p className="font-medium [overflow-wrap:anywhere]">{t.name}</p>
                <p className="min-w-0 text-xs text-ink-2 [overflow-wrap:anywhere]">{t.summary}</p>
              </Td>
              <Td label="Source" className="text-xs [overflow-wrap:anywhere]">
                {t.source.name}
              </Td>
              <Td label="Offered version" className="whitespace-nowrap font-mono text-xs">
                {t.current_version ?? <span className="font-sans text-muted">Nothing offered yet</span>}
              </Td>
              <Td label="Status" className="min-w-0">
                <TemplateChips t={t} />
              </Td>
              <Td label="Apps it needs" className="min-w-0">
                <NeedsApps t={t} nameOf={nameOf} />
              </Td>
              <Td label="Skill libraries it needs" className="min-w-0">
                <NeedsLibraries t={t} />
              </Td>
              <Td cell="actions" className="text-right">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={update.isPending}
                  onClick={() => {
                    setError(null);
                    update.mutate(
                      { templateId: t.template_id, body: { offered: !t.offered } },
                      { onError: (e) => setError(e) },
                    );
                  }}
                  aria-label={t.offered ? `Withdraw ${t.name}` : `Offer ${t.name} again`}
                >
                  {t.offered ? "Withdraw" : "Offer again"}
                </Button>
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
    </div>
  );
}
