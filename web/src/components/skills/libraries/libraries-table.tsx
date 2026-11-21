"use client";

import Link from "next/link";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ErrorNotice } from "@/components/ui/notice";
import {
  shortSha,
  useCheckLibraryNow,
  useDeleteSkillLibrary,
  usePatchSkillLibrary,
  type LibraryOwner,
  type SkillLibrary,
} from "@/lib/api/skill-libraries";
import { copyLabel, useCopy } from "@/lib/use-copy";
import { fmtDate } from "@/lib/utils";

import { EditLibraryDialog } from "./edit-library-dialog";
import { LibraryStatus, needsAddress } from "./library-status";

/** The repository (`host/path`, monospace, plain text) with a button that copies the address Aurora clones. */
function Repository({ library: l }: { library: SkillLibrary }) {
  const { state, copy } = useCopy();
  if (!l.repo) return <span className="text-muted">—</span>;
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      <span className="min-w-0 font-mono text-xs [overflow-wrap:anywhere]">{l.repo}</span>
      {l.url ? (
        // The name starts with the visible words ("Copy address"); while it says "Copied" the visible word names it.
        <Button type="button" variant="ghost" size="sm" aria-label={state === "idle" ? `Copy address of ${l.label}` : undefined} onClick={() => copy(l.url as string)}>
          {copyLabel(state, "Copy address")}
        </Button>
      ) : null}
    </div>
  );
}

const inUse = (l: SkillLibrary) => (l.skill_counts ? `${l.skill_counts.agent} agent · ${l.skill_counts.workspace} workspace · ${l.skill_counts.org} org` : "—");
const total = (l: SkillLibrary) => (l.skill_counts ? l.skill_counts.agent + l.skill_counts.workspace + l.skill_counts.org : 0);

/**
 * The libraries of one level (D-450, C-1129): the organization's own (`scope="org"`), the instance's own
 * (`scope="instance"`, with "Organizations using it") or the instance libraries an organization sees (`shared`:
 * read-only, with **Browse** instead of the manager's actions). Below 900 px each row is a card. `orgId` is the
 * organization whose list changes (`null` for the instance's own).
 */
export function LibrariesTable({
  scope,
  orgId,
  rows,
  canManage,
  caption,
  reviewHref,
  onBrowse,
}: {
  scope: "org" | "instance";
  orgId: LibraryOwner;
  rows: SkillLibrary[];
  canManage: boolean;
  caption: string;
  /** Where the review list is: the number of pending updates links there when it is above zero. */
  reviewHref?: string;
  /** Instance libraries listed in an organization: **Browse** opens Add from library in read mode. */
  onBrowse?: (library: SkillLibrary) => void;
}) {
  const patch = usePatchSkillLibrary(orgId);
  const check = useCheckLibraryNow(orgId);
  const del = useDeleteSkillLibrary(orgId);
  const [removing, setRemoving] = useState<SkillLibrary | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const editing = rows.find((r) => r.id === editingId) ?? null;
  const failed = patch.error ?? check.error ?? del.error;
  const instance = scope === "instance";
  return (
    <div className="space-y-3">
      <ErrorNotice error={failed} />
      <DataTable caption={caption} stackBelow={900}>
        <thead>
          <tr>
            <Th>Label</Th>
            <Th>Repository</Th>
            <Th>Branch or tag</Th>
            <Th>Folder</Th>
            <Th>Status</Th>
            <Th>Last checked</Th>
            <Th>Skills in use</Th>
            <Th>Pending updates</Th>
            {instance ? <Th>Organizations using it</Th> : null}
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((l) => {
            const paused = l.status === "paused";
            const pending = l.pending_updates ?? 0;
            return (
              <tr key={l.id}>
                <Td cell="title" className="min-w-0 font-medium [overflow-wrap:anywhere]">
                  {l.label}
                </Td>
                <Td label="Repository" className="min-w-0">
                  <Repository library={l} />
                </Td>
                <Td label="Branch or tag" className="font-mono text-xs">
                  {l.ref ?? <span className="font-sans">Default branch</span>}
                </Td>
                <Td label="Folder" className="font-mono text-xs">
                  {l.path ? l.path : <span className="font-sans">Repository root</span>}
                </Td>
                <Td label="Status">
                  <LibraryStatus library={l} />
                </Td>
                <Td label="Last checked" className="whitespace-nowrap text-xs">
                  {fmtDate(l.last_refresh_at, "Never")}
                  {l.last_head_sha ? <span className="ml-1.5 font-mono text-muted">{shortSha(l.last_head_sha)}</span> : null}
                </Td>
                <Td label="Skills in use" className="text-xs">
                  {inUse(l)}
                </Td>
                <Td label="Pending updates">
                  {pending > 0 && reviewHref ? (
                    <Link href={reviewHref} className="text-link underline" aria-label={`${pending} pending ${pending === 1 ? "update" : "updates"} of ${l.label}, review them`}>
                      {pending}
                    </Link>
                  ) : (
                    pending
                  )}
                </Td>
                {instance ? (
                  <Td label="Organizations using it">{l.orgs_using ?? 0}</Td>
                ) : null}
                <Td cell="actions" className="text-right">
                  {canManage ? (
                    <div className="flex flex-wrap justify-end gap-1">
                      {needsAddress(l) ? (
                        <Button size="sm" variant="outline" onClick={() => setEditingId(l.id)} aria-label={`Set the address of ${l.label}`}>
                          Set address
                        </Button>
                      ) : (
                        <Button size="sm" variant="outline" disabled={check.isPending || paused} onClick={() => check.mutate(l.id)} aria-label={`Check now ${l.label}`}>
                          Check now
                        </Button>
                      )}
                      <Button size="sm" variant="outline" onClick={() => setEditingId(l.id)} aria-label={`Edit ${l.label}`}>
                        Edit
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={patch.isPending}
                        onClick={() => patch.mutate({ id: l.id, body: { status: paused ? "active" : "paused" } })}
                        aria-label={`${paused ? "Resume" : "Pause"} ${l.label}`}
                      >
                        {paused ? "Resume" : "Pause"}
                      </Button>
                      <Button size="sm" variant="danger" onClick={() => setRemoving(l)} aria-label={`Delete ${l.label}`}>
                        Delete
                      </Button>
                    </div>
                  ) : onBrowse && l.status === "active" ? (
                    <Button size="sm" variant="outline" onClick={() => onBrowse(l)} aria-label={`Browse ${l.label}`}>
                      Browse
                    </Button>
                  ) : null}
                </Td>
              </tr>
            );
          })}
        </tbody>
      </DataTable>
      <EditLibraryDialog library={editing} orgId={orgId} open={!!editing} onOpenChange={(o) => !o && setEditingId(null)} />
      <ConfirmDialog
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Delete ${removing?.label ?? "library"}?`}
        description={
          instance
            ? `${removing ? total(removing) : 0} skills added from it, in every organization, keep their content and become editable skills.`
            : `${removing ? total(removing) : 0} skills keep their content and become editable skills.`
        }
        confirmLabel="Delete library"
        danger
        busy={del.isPending}
        onConfirm={() => removing && del.mutate(removing.id, { onSuccess: () => setRemoving(null) })}
      />
    </div>
  );
}
