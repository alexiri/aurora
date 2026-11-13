"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useState, type FormEvent } from "react";

import { OrgGate } from "@/components/org/org-gate";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { ApiError } from "@/lib/api/client";
import { useApps, useConnections } from "@/lib/api/apps";
import { errorMessage } from "@/lib/api/errors";
import {
  shortSha,
  useCheckLibraryNow,
  useCreateSkillLibrary,
  useDeleteSkillLibrary,
  usePatchSkillLibrary,
  useSkillLibraries,
  type SkillLibrary,
} from "@/lib/api/skill-libraries";

const STATUS: Record<string, { tone: "accent" | "neutral" | "pending" | "danger"; label: string }> = {
  active: { tone: "accent", label: "Active" },
  paused: { tone: "neutral", label: "Paused" },
  error: { tone: "danger", label: "Error" },
  over_budget: { tone: "pending", label: "Pending updates over budget" },
};

function when(iso: string | null | undefined): string {
  return iso ? new Date(iso).toLocaleString() : "Never";
}

/** Skills → Libraries (docs/14, D-148): connect a repository as a source of skills; org owners and admins. */
export function SkillLibrariesPage() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <Libraries slug={slug} orgId={org.id} />}
    </OrgGate>
  );
}

function Libraries({ slug, orgId }: { slug: string; orgId: string }) {
  const libs = useSkillLibraries(orgId);
  const [connect, setConnect] = useState(false);
  const disabled = libs.error instanceof ApiError && libs.error.code === "skill_libraries_disabled";
  return (
    <div className="max-w-page">
      <PageHeader
        title="Skill libraries"
        description="Connect a git repository as a source of skills. Skills are added to an agent, a workspace or the organization, pinned to a commit and read-only."
        actions={
          <>
            <Button asChild variant="outline">
              <Link href={`/o/${slug}/skills`}>Organization skills</Link>
            </Button>
            {disabled ? null : <Button onClick={() => setConnect(true)}>Connect a repository</Button>}
          </>
        }
      />
      {libs.isPending ? <Loading /> : null}
      {disabled ? <EmptyState title="Skill libraries are turned off on this instance" /> : null}
      {libs.isError && !disabled ? <ErrorState error={libs.error} onRetry={() => libs.refetch()} /> : null}
      {libs.data && libs.data.items.length === 0 ? (
        <EmptyState title="No library connected yet">
          <p className="text-sm text-ink-2">Connect a repository of GitHub or GitLab with an organization-level connection (use a read-only token).</p>
        </EmptyState>
      ) : null}
      {libs.data && libs.data.items.length > 0 ? <LibraryTable orgId={orgId} slug={slug} items={libs.data.items} /> : null}
      <ConnectDialog orgId={orgId} open={connect} onOpenChange={setConnect} />
    </div>
  );
}

function LibraryTable({ orgId, slug, items }: { orgId: string; slug: string; items: SkillLibrary[] }) {
  const patch = usePatchSkillLibrary(orgId);
  const check = useCheckLibraryNow(orgId);
  const del = useDeleteSkillLibrary(orgId);
  const [removing, setRemoving] = useState<SkillLibrary | null>(null);
  const failed = patch.error ?? check.error ?? del.error;
  const total = (l: SkillLibrary) => (l.skill_counts ? l.skill_counts.agent + l.skill_counts.workspace + l.skill_counts.org : 0);
  return (
    <div className="space-y-3">
      {failed ? (
        <Notice tone="danger" role="alert">
          {errorMessage(failed)}
        </Notice>
      ) : null}
      <DataTable caption="Skill libraries">
        <thead>
          <tr>
            <Th>Label</Th>
            <Th>Repository</Th>
            <Th>Ref</Th>
            <Th>Folder</Th>
            <Th>Status</Th>
            <Th>Last checked</Th>
            <Th>Skills in use</Th>
            <Th>Pending updates</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {items.map((l) => {
            const st = STATUS[l.status] ?? STATUS.error;
            const needsReconnect = l.connection?.status === "reauth_required" || l.last_error?.includes("reauth_required");
            return (
              <tr key={l.id}>
                <Td className="font-medium">{l.label}</Td>
                <Td className="font-mono text-xs">{l.repo}</Td>
                <Td className="font-mono text-xs">{l.ref ?? "default branch"}</Td>
                <Td className="font-mono text-xs">{l.path ? l.path : "/"}</Td>
                <Td>
                  <StatusDot tone={st.tone}>{st.label}</StatusDot>
                  {l.last_error ? <p className="mt-1 max-w-xs whitespace-normal text-xs text-danger-ink">{l.last_error}</p> : null}
                  {needsReconnect ? (
                    <p className="mt-1 text-xs">
                      <Link href={`/o/${slug}/credentials`} className="text-link underline">
                        Reconnect
                      </Link>
                    </p>
                  ) : null}
                  {l.last_head_sha ? <p className="mt-1 font-mono text-xs text-muted">{shortSha(l.last_head_sha)}</p> : null}
                </Td>
                <Td className="whitespace-nowrap text-xs">{when(l.last_refresh_at)}</Td>
                <Td className="text-xs">
                  {l.skill_counts ? `${l.skill_counts.agent} agent · ${l.skill_counts.workspace} workspace · ${l.skill_counts.org} org` : "—"}
                </Td>
                <Td>{l.pending_updates ?? 0}</Td>
                <Td className="text-right">
                  <div className="flex flex-wrap justify-end gap-1">
                    <Button size="sm" variant="outline" disabled={check.isPending || l.status === "paused"} onClick={() => check.mutate(l.id)} aria-label={`Check now ${l.label}`}>
                      Check now
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={patch.isPending}
                      onClick={() => patch.mutate({ id: l.id, body: { status: l.status === "paused" ? "active" : "paused" } })}
                      aria-label={`${l.status === "paused" ? "Resume" : "Pause"} ${l.label}`}
                    >
                      {l.status === "paused" ? "Resume" : "Pause"}
                    </Button>
                    <Button size="sm" variant="danger" onClick={() => setRemoving(l)} aria-label={`Delete ${l.label}`}>
                      Delete
                    </Button>
                  </div>
                </Td>
              </tr>
            );
          })}
        </tbody>
      </DataTable>
      <ConfirmDialog
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Delete ${removing?.label ?? "library"}?`}
        description={`${removing ? total(removing) : 0} skills keep their content and become editable skills.`}
        confirmLabel="Delete library"
        danger
        busy={del.isPending}
        onConfirm={() => removing && del.mutate(removing.id, { onSuccess: () => setRemoving(null) })}
      />
    </div>
  );
}

function ConnectDialog({ orgId, open, onOpenChange }: { orgId: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const create = useCreateSkillLibrary(orgId);
  const apps = useApps(undefined);
  const conns = useConnections({ kind: "organization", id: orgId });
  const serving = new Set((apps.data?.items ?? []).filter((a) => a.serves_library).map((a) => a.app_id));
  const usable = (conns.data?.items ?? []).filter((c) => serving.has(c.app_id) && c.scope === "organization" && c.status === "active");
  const [label, setLabel] = useState("");
  const [connection, setConnection] = useState("");
  const [repo, setRepo] = useState("");
  const [ref, setRef] = useState("");
  const [path, setPath] = useState("");
  const [err, setErr] = useState("");
  const chosen = connection || usable[0]?.id || "";

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!label.trim() || !repo.trim() || !chosen) return setErr("Enter a label, pick a connection and enter the repository (owner/name).");
    setErr("");
    create.mutate(
      { label: label.trim(), connection_id: chosen, repo: repo.trim(), ref: ref.trim() || null, path: path.trim() || null },
      {
        onSuccess: () => {
          onOpenChange(false);
          setLabel("");
          setRepo("");
          setRef("");
          setPath("");
        },
      },
    );
  }

  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        onOpenChange(o);
        if (!o) create.reset();
      }}
      title="Connect a repository"
      description="Aurora reads the repository through an organization-level connection and only ever sends GET requests."
      submitLabel="Connect"
      busy={create.isPending}
      submitDisabled={usable.length === 0}
      onSubmit={submit}
    >
      <Notice role="status">Use a read-only token (GitHub: Contents read; GitLab: read_repository). A token that can write is more access than Aurora needs.</Notice>
      {conns.isSuccess && usable.length === 0 ? (
        <Notice tone="pending" role="status">
          No organization-level connection of an app that can serve a library (GitHub or GitLab) is active. Connect one under Credentials first.
        </Notice>
      ) : null}
      <Field id="lib-label" label="Label">
        <input id="lib-label" className={inputClass} value={label} maxLength={80} onChange={(e) => setLabel(e.target.value)} />
      </Field>
      <Field id="lib-conn" label="Connection">
        <select id="lib-conn" className={inputClass} value={chosen} onChange={(e) => setConnection(e.target.value)}>
          {usable.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label} ({c.app_id})
            </option>
          ))}
        </select>
      </Field>
      <Field id="lib-repo" label="Repository" hint="owner/name, for example acme/skills">
        <input id="lib-repo" className={inputClass} value={repo} onChange={(e) => setRepo(e.target.value)} />
      </Field>
      <Field id="lib-ref" label="Branch or tag (optional)" hint="Default: the repository's default branch.">
        <input id="lib-ref" className={inputClass} value={ref} onChange={(e) => setRef(e.target.value)} />
      </Field>
      <Field id="lib-path" label="Folder (optional)" hint="The folder that holds the skill folders, for example skills. Default: the repository root.">
        <input id="lib-path" className={inputClass} value={path} onChange={(e) => setPath(e.target.value)} />
      </Field>
      {err ? (
        <Notice tone="danger" role="alert">
          {err}
        </Notice>
      ) : null}
      {create.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(create.error)}
        </Notice>
      ) : null}
    </FormDialog>
  );
}
