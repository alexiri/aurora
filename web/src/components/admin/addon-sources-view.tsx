"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import {
  checkSummary,
  INTERVALS,
  intervalLabel,
  useAddAddonSource,
  useAddonSources,
  useCheckAddonSource,
  useConfirmAddonKey,
  usePreviewAddonSource,
  useRemoveAddonSource,
  useUpdateAddonSource,
  type AddonEntry,
  type AddonSource,
  type AddonSourceCheckResult,
  type Interval,
} from "@/lib/api/addon-sources";
import { errorMessage } from "@/lib/api/errors";

/** The documented format of an addon repository (docs/22-addon-repos.md in the repository). */
const FORMAT_DOC = "docs/22-addon-repos.md";

function Fingerprint({ value, label = "Signing key fingerprint" }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <code aria-label={label} className="rounded-control bg-subtle px-2 py-0.5 font-mono text-xs text-ink">
        {value}
      </code>
      <Button
        size="sm"
        variant="outline"
        aria-label={`Copy ${label.toLowerCase()}`}
        onClick={() => {
          void navigator.clipboard?.writeText(value).then(() => setCopied(true));
        }}
      >
        {copied ? "Copied" : "Copy"}
      </Button>
    </span>
  );
}

export function StatusBadge({ source }: { source: AddonSource }) {
  if (source.status === "signing_key_changed") return <Badge tone="danger">Signing key changed</Badge>;
  if (source.status === "check_failed") return <Badge tone="danger">Check failed</Badge>;
  return <Badge tone="accent">OK</Badge>;
}

function AddonList({ addons }: { addons: AddonEntry[] }) {
  if (addons.length === 0) return <p className="text-sm text-muted">The repository lists no addons.</p>;
  return (
    <ul aria-label="Addons" className="space-y-3">
      {addons.map((a) => (
        <li key={a.id} className="rounded-control border border-line p-3">
          <p className="font-medium text-ink">
            {a.name} <span className="font-mono text-xs text-muted">{a.id}</span>
          </p>
          {a.description ? <p className="text-sm text-ink-2">{a.description}</p> : null}
          <ul className="mt-2 flex flex-wrap gap-2">
            {a.versions.map((v) => (
              <li key={v.version} className="flex flex-wrap items-center gap-1 text-xs">
                <code className="rounded-control bg-subtle px-1.5 py-0.5 font-mono">{v.version}</code>
                {v.installed_version === v.version ? <Badge tone="accent">Installed</Badge> : null}
                {v.update_available ? <Badge tone="pending">Update available</Badge> : null}
                {!v.ingestable ? (
                  <Badge tone="neutral">{`Needs a newer Aurora${v.min_core ? ` (${v.min_core})` : ""}`}</Badge>
                ) : null}
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

function IntervalSelect({ id, value, onChange }: { id: string; value: Interval; onChange: (v: Interval) => void }) {
  return (
    <select id={id} className={inputClass} value={value} onChange={(e) => onChange(e.target.value as Interval)}>
      {INTERVALS.map((i) => (
        <option key={i} value={i}>
          {intervalLabel(i)}
        </option>
      ))}
    </select>
  );
}

/** Add flow: paste a URL → review the repository and its signing key → confirm (trusts the key). */
export function AddSourceDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const [url, setUrl] = useState("");
  const [interval, setIntervalValue] = useState<Interval>("1d");
  const preview = usePreviewAddonSource();
  const add = useAddAddonSource();
  const p = preview.data;
  const close = (o: boolean) => {
    if (!o) {
      preview.reset();
      add.reset();
      setUrl("");
    }
    onOpenChange(o);
  };
  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent
        size="xl"
        title="Add an addon source"
        description="A repository of third-party app packages. Nothing is registered until you have reviewed its signing key."
      >
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            add.reset();
            preview.mutate(url.trim());
          }}
        >
          <Field
            id="addon-url"
            label="Repository or manifest URL"
            hint="https only. A GitHub or GitLab repository page works; so does the direct link to aurora-addons.yaml."
          >
            <input
              id="addon-url"
              required
              type="url"
              className={inputClass}
              placeholder="https://github.com/acme/aurora-addons"
              value={url}
              onChange={(e) => {
                setUrl(e.target.value);
                preview.reset();
              }}
            />
          </Field>
          <p className="text-xs text-muted">
            What a repository publishes is described in the format documentation (<code>{FORMAT_DOC}</code>).
          </p>
          {preview.isError ? (
            <Notice tone="danger" role="alert">
              {errorMessage(preview.error)}
            </Notice>
          ) : null}
          {!p ? (
            <div className="flex justify-end">
              <Button type="submit" disabled={preview.isPending || url.trim() === ""}>
                {preview.isPending ? "Fetching…" : "Review repository"}
              </Button>
            </div>
          ) : null}
        </form>
        {p ? (
          <div className="mt-5 space-y-4" aria-label="Repository review">
            <div>
              <h3 className="font-display text-base font-semibold text-ink">{p.repo.name}</h3>
              {p.repo.description ? <p className="text-sm text-ink-2">{p.repo.description}</p> : null}
              <dl className="mt-2 grid gap-x-4 gap-y-1 text-sm sm:grid-cols-[auto_1fr]">
                <dt className="text-muted">Publisher</dt>
                <dd className="font-mono">{p.publisher}</dd>
                {p.repo.homepage ? (
                  <>
                    <dt className="text-muted">Homepage</dt>
                    <dd className="break-all">{p.repo.homepage}</dd>
                  </>
                ) : null}
                {p.repo.contact ? (
                  <>
                    <dt className="text-muted">Contact</dt>
                    <dd className="break-all">{p.repo.contact}</dd>
                  </>
                ) : null}
              </dl>
            </div>
            <Notice tone="pending" role="status" className="space-y-2">
              <p className="font-medium">Signing key — check this fingerprint with the publisher</p>
              <p className="text-lg">
                <Fingerprint value={p.key_fingerprint} />
              </p>
              {p.key_description ? <p className="text-xs">{p.key_description}</p> : null}
              <p className="text-xs">
                Adding the source trusts this key for publisher <code>{p.publisher}</code>: every package it
                signs can be installed here after your approval. Packages never activate on their own.
              </p>
            </Notice>
            <AddonList addons={p.addons} />
            <p className="text-sm text-ink-2" role="status">
              {p.packages_new === 0
                ? "Nothing new to ingest: this instance already has what the repository lists."
                : `${p.packages_new} ${p.packages_new === 1 ? "package is" : "packages are"} new to this instance and will be added for review.`}
            </p>
            <Field id="addon-interval" label="Check for updates">
              <IntervalSelect id="addon-interval" value={interval} onChange={setIntervalValue} />
            </Field>
            {add.isError ? (
              <Notice tone="danger" role="alert">
                {errorMessage(add.error)}
              </Notice>
            ) : null}
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button
                disabled={add.isPending}
                onClick={() =>
                  add.mutate({ url: p.url, update_interval: interval }, { onSuccess: () => close(false) })
                }
              >
                {add.isPending ? "Adding…" : "Trust this key and add source"}
              </Button>
            </div>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function ConfirmKeyDialog({ source, onClose }: { source: AddonSource | null; onClose: () => void }) {
  const confirm = useConfirmAddonKey();
  return (
    <Dialog open={source !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        size="md"
        title="This repository's signing key changed"
        description="New versions are refused until you confirm. Packages already installed stay as they are."
      >
        {source ? (
          <div className="space-y-4">
            <dl className="grid gap-3 sm:grid-cols-2">
              <div>
                <dt className="text-xs uppercase tracking-label text-muted">Trusted key until now</dt>
                <dd className="mt-1">
                  <Fingerprint value={source.key_fingerprint} label="Old fingerprint" />
                </dd>
              </div>
              <div>
                <dt className="text-xs uppercase tracking-label text-muted">Key the repository now publishes</dt>
                <dd className="mt-1">
                  <Fingerprint value={source.new_key_fingerprint ?? ""} label="New fingerprint" />
                </dd>
              </div>
            </dl>
            <Notice tone="pending">
              Confirm only if the publisher told you the key was rotated. Confirming moves the trusted key of
              publisher <code>{source.publisher}</code> to the new key, and the repository is checked again.
            </Notice>
            {confirm.isError ? (
              <Notice tone="danger" role="alert">
                {errorMessage(confirm.error)}
              </Notice>
            ) : null}
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button
                variant="danger"
                disabled={confirm.isPending || !source.new_key_fingerprint}
                onClick={() =>
                  source.new_key_fingerprint &&
                  confirm.mutate(
                    { id: source.id, new_key_fingerprint: source.new_key_fingerprint },
                    { onSuccess: onClose },
                  )
                }
              >
                Trust the new key
              </Button>
            </div>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function ScheduleDialog({ source, onClose }: { source: AddonSource | null; onClose: () => void }) {
  const update = useUpdateAddonSource();
  const [value, setValue] = useState<Interval | null>(null);
  const current = (value ?? (source?.update_interval as Interval | undefined) ?? "1d") as Interval;
  return (
    <Dialog
      open={source !== null}
      onOpenChange={(o) => {
        if (!o) {
          setValue(null);
          update.reset();
          onClose();
        }
      }}
    >
      <DialogContent title="Check schedule" description={source ? `How often ${source.name} is checked for new versions.` : ""}>
        {source ? (
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              update.mutate({ id: source.id, update_interval: current }, { onSuccess: onClose });
            }}
          >
            <Field id="addon-schedule" label="Check for updates">
              <IntervalSelect id="addon-schedule" value={current} onChange={setValue} />
            </Field>
            <p className="text-xs text-muted">“Check now” works whatever the schedule is.</p>
            {update.isError ? (
              <Notice tone="danger" role="alert">
                {errorMessage(update.error)}
              </Notice>
            ) : null}
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button type="submit" disabled={update.isPending}>
                Save schedule
              </Button>
            </div>
          </form>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function DetailsDialog({ source, onClose }: { source: AddonSource | null; onClose: () => void }) {
  return (
    <Dialog open={source !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent size="xl" title={source ? `${source.name} — addons` : "Addons"} description="From the last accepted manifest.">
        {source ? (
          <div className="space-y-4">
            <AddonList addons={source.addons} />
            {source.removed_upstream.length > 0 ? (
              <Notice tone="neutral" role="status" className="space-y-1">
                <p className="font-medium">Removed upstream</p>
                <p className="text-xs">These versions are installed here but the repository no longer lists them. They keep working.</p>
                <ul className="font-mono text-xs">
                  {source.removed_upstream.map((r) => (
                    <li key={`${r.package_id}@${r.version}`}>{`${r.package_id} ${r.version}`}</li>
                  ))}
                </ul>
              </Notice>
            ) : null}
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

export function AddonSourcesView() {
  const q = useAddonSources();
  const check = useCheckAddonSource();
  const remove = useRemoveAddonSource();
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<AddonSource | null>(null);
  const [rekey, setRekey] = useState<AddonSource | null>(null);
  const [schedule, setSchedule] = useState<AddonSource | null>(null);
  const [details, setDetails] = useState<AddonSource | null>(null);
  const [running, setRunning] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<{ result: AddonSourceCheckResult; name: string } | null>(null);
  const sources = q.data?.items ?? [];
  const updates = sources.reduce(
    (n, s) => n + s.addons.filter((a) => a.versions.some((v) => v.update_available && v.ingestable)).length,
    0,
  );
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="Addon sources"
        description="Third-party repositories of app packages. Each is trusted through its own signing key; every package still waits for your approval."
        className="mb-2"
        actions={<Button onClick={() => setAdding(true)}>Add source</Button>}
      />
      {updates > 0 ? (
        <Notice tone="pending" role="status">
          {`${updates} ${updates === 1 ? "addon has" : "addons have"} a newer version in its repository.`}
        </Notice>
      ) : null}
      {outcome ? (
        <Notice tone={outcome.result.status === "failed" ? "danger" : "accent"} role="status">
          {checkSummary(outcome.result, outcome.name)}
          {outcome.result.failed_versions.length > 0 ? (
            <ul className="mt-1 list-disc pl-5 text-xs">
              {outcome.result.failed_versions.map((f) => (
                <li key={`${f.package_id}@${f.version}`}>{`${f.package_id} ${f.version}: ${f.reason}`}</li>
              ))}
            </ul>
          ) : null}
        </Notice>
      ) : null}
      {check.isError ? (
        <Notice tone="danger" role="alert">
          {errorMessage(check.error)}
        </Notice>
      ) : null}
      <Section id="addon-list" title="Sources">
        {q.isPending ? <Loading /> : null}
        {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
        {q.data && sources.length === 0 ? (
          <EmptyState title="No addon sources" action={<Button onClick={() => setAdding(true)}>Add source</Button>}>
            Add a repository to offer third-party apps. Apps you add this way follow the same review and
            approval as any uploaded package.
          </EmptyState>
        ) : null}
        {sources.length > 0 ? (
          <DataTable caption="Addon sources">
            <thead>
              <tr>
                <Th>Source</Th>
                <Th>Publisher</Th>
                <Th>Signing key</Th>
                <Th>Schedule</Th>
                <Th>Last checked</Th>
                <Th>Addons</Th>
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {sources.map((s) => (
                <tr key={s.id} id={s.id}>
                  <Td>
                    <span className="font-medium">{s.name}</span>
                    <div className="max-w-xs break-all text-xs text-muted">{s.url}</div>
                  </Td>
                  <Td className="font-mono text-xs">{s.publisher}</Td>
                  <Td>
                    <Fingerprint value={s.key_fingerprint} />
                  </Td>
                  <Td className="whitespace-nowrap text-ink-2">{intervalLabel(s.update_interval)}</Td>
                  <Td>
                    <div className="space-y-1">
                      <div className="whitespace-nowrap text-ink-2">
                        {s.last_checked_at ? fmtDate(s.last_checked_at) : "Not yet"}
                      </div>
                      <StatusBadge source={s} />
                      {s.status === "check_failed" && s.check_failed_reason ? (
                        <p className="max-w-xs text-xs text-danger-ink">{s.check_failed_reason}</p>
                      ) : null}
                    </div>
                  </Td>
                  <Td>
                    <Button size="sm" variant="outline" aria-label={`Addons of ${s.name}`} onClick={() => setDetails(s)}>
                      {s.addon_count}
                    </Button>
                  </Td>
                  <Td>
                    <div className="flex flex-wrap gap-1.5">
                      {s.status === "signing_key_changed" ? (
                        <Button size="sm" variant="danger" aria-label={`Review the new signing key of ${s.name}`} onClick={() => setRekey(s)}>
                          Review key
                        </Button>
                      ) : null}
                      <Button
                        size="sm"
                        variant="outline"
                        aria-label={`Check ${s.name} now`}
                        disabled={running === s.id}
                        onClick={() => {
                          setRunning(s.id);
                          setOutcome(null);
                          check.mutate(s.id, {
                            onSuccess: (result) => setOutcome({ result, name: s.name }),
                            onSettled: () => setRunning(null),
                          });
                        }}
                      >
                        {running === s.id ? "Checking…" : "Check now"}
                      </Button>
                      <Button size="sm" variant="outline" aria-label={`Edit the schedule of ${s.name}`} onClick={() => setSchedule(s)}>
                        Schedule
                      </Button>
                      <Button size="sm" variant="danger" aria-label={`Remove ${s.name}`} onClick={() => setRemoving(s)}>
                        Remove
                      </Button>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        ) : null}
      </Section>
      <AddSourceDialog open={adding} onOpenChange={setAdding} />
      <ConfirmKeyDialog source={rekey} onClose={() => setRekey(null)} />
      <ScheduleDialog source={schedule} onClose={() => setSchedule(null)} />
      <DetailsDialog source={details} onClose={() => setDetails(null)} />
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(o) => {
          if (!o) {
            setRemoving(null);
            remove.reset();
          }
        }}
        title="Remove addon source"
        description={
          removing
            ? `Packages already installed from ${removing.name} stay installed and keep working, and the trusted key stays trusted; Aurora stops checking this repository for new versions. Removal is refused while any of its packages is active.`
            : undefined
        }
        confirmLabel="Remove source"
        danger
        busy={remove.isPending}
        onConfirm={() => removing && remove.mutate(removing.id, { onSuccess: () => setRemoving(null) })}
      >
        {remove.isError ? (
          <Notice tone="danger" role="alert" className="mb-2">
            {errorMessage(remove.error)}
          </Notice>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}
