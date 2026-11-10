"use client";

import { useEffect, useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions, FormSection } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { Section } from "@/components/ui/section";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useAdminOrgs } from "@/lib/api/freeze";
import {
  usePlacement,
  usePlacementChoices,
  usePlacementPreview,
  useSavePlacement,
  type PlacementMode,
  type PlacementView as Placement,
  type PutPlacement,
} from "@/lib/api/placement";

type SharedRole = "member" | "approver";
type PersonalRole = "owner" | "approver" | "member";
type OrgRole = "member" | "admin";

const NEW_ORG = "__new__";

const MODES: { value: PlacementMode; label: string }[] = [
  { value: "shared_org", label: "Shared organization" },
  { value: "personal_org", label: "Own organization" },
  { value: "none", label: "No placement" },
];

const MODE_HELP: Record<PlacementMode, string> = {
  shared_org: "New users join one organization you choose, as members, and can get a workspace of their own.",
  personal_org: "Every new user gets their own organization and owns it. This is how sign-up worked before placement existed.",
  none: "Nobody is placed automatically. People enter only by invitation, by a role from the sign-in provider, or by a provider's JIT rule.",
};

/** Admin › Sign-up & placement (P4-ONB-1, D-140): where new users land. */
export function PlacementView({ embedded = false, onSaved }: { embedded?: boolean; onSaved?: () => void } = {}) {
  const q = usePlacement();
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  // Re-key on the saved revision so the form always starts from what the server holds.
  return <PlacementForm key={q.data.updated_at ?? "unsaved"} saved={q.data} embedded={embedded} onSaved={onSaved} />;
}

function useDebounced<T>(value: T, ms = 250): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

function PlacementForm({ saved, embedded, onSaved }: { saved: Placement; embedded: boolean; onSaved?: () => void }) {
  const save = useSavePlacement();
  const orgs = useAdminOrgs(false);

  // A fresh instance proposes the recommended choice: shared organization "Users" (an existing
  // one, else a new one), personal workspaces. Nothing is sent until the admin saves.
  const [mode, setMode] = useState<PlacementMode>(saved.saved ? saved.mode : "shared_org");
  const [picked, setPicked] = useState<string | null>(null);
  const proposed = saved.saved
    ? (saved.default_org?.id ?? "")
    : orgs.data
      ? (orgs.data.items.find((o) => o.slug === "users" || o.name === "Users")?.id ?? NEW_ORG)
      : "";
  const orgChoice = picked ?? proposed;
  const setOrgChoice = setPicked;
  const [newOrgName, setNewOrgName] = useState("Users");
  const [orgRole, setOrgRole] = useState<OrgRole>(saved.default_org_role);
  const [shared, setShared] = useState<Record<string, SharedRole>>(
    Object.fromEntries(saved.shared_workspaces.map((w) => [w.workspace_id, w.role])),
  );
  const [personal, setPersonal] = useState(saved.saved ? saved.personal_workspace.enabled : true);
  const [personalRole, setPersonalRole] = useState<PersonalRole>(saved.personal_workspace.role);
  const [template, setTemplate] = useState(saved.personal_workspace.name_template);
  const [sample, setSample] = useState("bobj");
  const [starters, setStarters] = useState<Set<string>>(
    new Set(saved.starter_grants.map((g) => `${g.kind}:${g.id}`)),
  );
  const [done, setDone] = useState(false);

  const creating = orgChoice === NEW_ORG;
  const orgId = mode === "shared_org" && orgChoice && !creating ? orgChoice : null;
  const choices = usePlacementChoices(orgId);
  const preview = usePlacementPreview(useDebounced(template), useDebounced(sample));

  const errs = save.error ? fieldErrors(save.error) : {};

  function toggle<T>(set: Set<T>, key: T, on: boolean): Set<T> {
    const n = new Set(set);
    if (on) n.add(key);
    else n.delete(key);
    return n;
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    setDone(false);
    const body: PutPlacement = { mode };
    if (mode === "shared_org") {
      if (creating) body.create_default_org = newOrgName.trim();
      else body.default_org_id = orgChoice;
      body.default_org_role = orgRole;
      body.shared_workspaces = Object.entries(shared).map(([workspace_id, role]) => ({ workspace_id, role }));
      body.personal_workspace = { enabled: personal, role: personalRole, name_template: template };
      body.starter_grants = personal
        ? [...starters].map((s) => {
            const [kind, id] = s.split(":");
            return { kind: kind as "secret" | "connection", id };
          })
        : [];
    }
    save.mutate(body, {
      onSuccess: () => {
        setDone(true);
        onSaved?.();
      },
    });
  }

  const shownSecrets = choices.data?.secrets ?? [];
  const llm = shownSecrets.filter((s) => s.kind === "llm");
  const otherSecrets = shownSecrets.filter((s) => s.kind !== "llm");
  const connections = choices.data?.connections ?? [];

  return (
    <div className="max-w-[1160px] space-y-6">
      {embedded ? null : (
        <PageHeader
          title="Sign-up & placement"
          description="Where people land the first time they sign in, and what they get on day one."
          className="mb-6"
        />
      )}
      {saved.warnings.includes("placement_unsaved") ? (
        <Notice tone="pending" role="status">
          Nothing is saved yet, so every new user still gets their own organization and owns it. Choose where people should
          land and save.
        </Notice>
      ) : saved.warnings.includes("per_user_orgs_active") ? (
        <Notice tone="pending" role="status">
          Every new user gets their own organization and owns it. Choose a shared organization if people should land together.
        </Notice>
      ) : null}
      {saved.warnings.includes("default_org_missing") ? (
        <Notice tone="danger" role="alert">
          The default organization was deleted, so nobody is placed automatically. Pick another one and save.
        </Notice>
      ) : null}

      <form onSubmit={submit} noValidate className="max-w-3xl space-y-5">
        <FormSection
          title="Where new users land"
          description={
            <>
              Sign-up is <strong>{saved.signup_mode.replace("_", "-")}</strong> (set in the Helm values). Placement applies to
              everyone who is admitted, however they were admitted.
            </>
          }
        >
          <Segmented label="Placement mode" value={mode} options={MODES} onChange={setMode} className="flex-wrap" />
          <p className="text-sm text-ink-2">{MODE_HELP[mode]}</p>
        </FormSection>

        {mode === "shared_org" ? (
          <>
            <FormSection title="Default organization">
              <Field id="pl-org" label="Organization" error={errs.default_org_id}>
                <select id="pl-org" className={inputClass} value={orgChoice} onChange={(e) => setOrgChoice(e.target.value)}>
                  <option value="" disabled>
                    Choose an organization
                  </option>
                  {(orgs.data?.items ?? []).map((o) => (
                    <option key={o.id} value={o.id}>
                      {o.name}
                    </option>
                  ))}
                  <option value={NEW_ORG}>Create a new organization…</option>
                </select>
              </Field>
              {creating ? (
                <Field id="pl-org-name" label="New organization name" hint="You become its owner." error={errs.name}>
                  <input
                    id="pl-org-name"
                    className={inputClass}
                    value={newOrgName}
                    maxLength={80}
                    autoComplete="off"
                    onChange={(e) => setNewOrgName(e.target.value)}
                  />
                </Field>
              ) : null}
              <Field id="pl-org-role" label="New users join as">
                <select id="pl-org-role" className={inputClass} value={orgRole} onChange={(e) => setOrgRole(e.target.value as OrgRole)}>
                  <option value="member">Member</option>
                  <option value="admin">Admin</option>
                </select>
              </Field>
            </FormSection>

            <FormSection
              title="Shared workspaces"
              description="Workspaces every new user joins. Personal workspaces are never listed here."
            >
              {creating ? (
                <p className="text-sm text-ink-2">Save once to create the organization, then pick its workspaces.</p>
              ) : choices.isPending && orgId ? (
                <Loading />
              ) : (choices.data?.workspaces ?? []).length === 0 ? (
                <p className="text-sm text-ink-2">This organization has no workspaces to share.</p>
              ) : (
                <ul className="space-y-3">
                  {(choices.data?.workspaces ?? []).map((w) => (
                    <li key={w.id} className="flex flex-wrap items-center justify-between gap-2">
                      <CheckboxField
                        id={`pl-ws-${w.id}`}
                        label={w.name}
                        hint={w.slug}
                        checked={w.id in shared}
                        onChange={(on) =>
                          setShared((s) => {
                            const n = { ...s };
                            if (on) n[w.id] = "member";
                            else delete n[w.id];
                            return n;
                          })
                        }
                      />
                      {w.id in shared ? (
                        <select
                          aria-label={`Role in ${w.name}`}
                          className={`${inputClass} !w-auto`}
                          value={shared[w.id]}
                          onChange={(e) => setShared((s) => ({ ...s, [w.id]: e.target.value as SharedRole }))}
                        >
                          <option value="member">Member</option>
                          <option value="approver">Approver</option>
                        </select>
                      ) : null}
                    </li>
                  ))}
                </ul>
              )}
            </FormSection>

            <FormSection
              title="Personal workspace"
              description="Each user gets a workspace of their own, with the default agent in it, created at their first sign-in."
            >
              <CheckboxField id="pl-personal" label="Give every user a workspace of their own" checked={personal} onChange={setPersonal} />
              {personal ? (
                <>
                  <Field id="pl-personal-role" label="Their role in it">
                    <select
                      id="pl-personal-role"
                      className={inputClass}
                      value={personalRole}
                      onChange={(e) => setPersonalRole(e.target.value as PersonalRole)}
                    >
                      <option value="owner">Owner</option>
                      <option value="approver">Approver</option>
                      <option value="member">Member</option>
                    </select>
                  </Field>
                  <Field
                    id="pl-template"
                    label="Workspace name"
                    hint="Variables: {preferred_username} {name} {email_local} {given_name} {family_name} {claim:path}. Separate fallbacks with |."
                    error={errs["personal_workspace/name_template"]}
                  >
                    <input
                      id="pl-template"
                      className={`${inputClass} font-mono`}
                      value={template}
                      maxLength={200}
                      autoComplete="off"
                      spellCheck={false}
                      onChange={(e) => setTemplate(e.target.value)}
                    />
                  </Field>
                  <Field id="pl-sample" label="Try it with a name" hint="Leave empty to use your own profile.">
                    <input
                      id="pl-sample"
                      className={inputClass}
                      value={sample}
                      maxLength={200}
                      autoComplete="off"
                      onChange={(e) => setSample(e.target.value)}
                    />
                  </Field>
                  <p role="status" aria-live="polite" className="text-sm text-ink-2" data-testid="placement-preview">
                    {preview.isError ? (
                      <span className="text-danger-ink">{errorMessage(preview.error)}</span>
                    ) : preview.data ? (
                      <>
                        {sample.trim() ? <q>{sample.trim()}</q> : "You"} → workspace <strong>{preview.data.name}</strong> (
                        <code className="font-mono">{preview.data.slug}</code>)
                        {preview.data.unresolved.length > 0 ? (
                          <span className="text-muted"> · no value here for {preview.data.unresolved.join(", ")}</span>
                        ) : null}
                      </>
                    ) : (
                      "Previewing…"
                    )}
                  </p>
                </>
              ) : null}
            </FormSection>

            {personal ? (
              <FormSection
                title="Starter grants"
                description="Credentials of the default organization that every new personal workspace's agent receives, so it works on day one."
              >
                {creating ? (
                  <p className="text-sm text-ink-2">Save once to create the organization, then pick its credentials.</p>
                ) : llm.length + otherSecrets.length + connections.length === 0 ? (
                  <p className="text-sm text-ink-2">The organization has no organization-level keys, secrets or connections yet.</p>
                ) : (
                  <>
                    {[
                      { title: "LLM keys", items: llm.map((s) => ({ k: `secret:${s.id}`, name: s.name, hint: "LLM key" })) },
                      { title: "Secrets", items: otherSecrets.map((s) => ({ k: `secret:${s.id}`, name: s.name, hint: s.kind })) },
                      { title: "Connections", items: connections.map((c) => ({ k: `connection:${c.id}`, name: c.label, hint: c.app_id })) },
                    ]
                      .filter((g) => g.items.length > 0)
                      .map((g) => (
                        <div key={g.title} className="space-y-2">
                          <h3 className="text-xs font-semibold uppercase tracking-[0.06em] text-muted">{g.title}</h3>
                          {g.items.map((i) => (
                            <CheckboxField
                              key={i.k}
                              id={`pl-grant-${i.k}`}
                              label={i.name}
                              hint={i.hint}
                              checked={starters.has(i.k)}
                              onChange={(on) => setStarters((s) => toggle(s, i.k, on))}
                            />
                          ))}
                        </div>
                      ))}
                  </>
                )}
              </FormSection>
            ) : null}
          </>
        ) : null}

        {save.isError ? (
          <Notice tone="danger" role="alert">
            {errorMessage(save.error)}
            {Object.values(errs).length > 0 ? (
              <ul className="mt-1 list-disc pl-5">
                {Object.entries(errs).map(([k, m]) => (
                  <li key={k}>
                    {k}: {m}
                  </li>
                ))}
              </ul>
            ) : null}
          </Notice>
        ) : null}
        <FormActions>
          <Button type="submit" disabled={save.isPending || (mode === "shared_org" && !orgChoice)}>
            {save.isPending ? "Saving…" : "Save placement"}
          </Button>
          {done ? (
            <span role="status" className="text-sm text-ink-2">
              Saved. New sign-ins follow it from now on.
            </span>
          ) : null}
        </FormActions>
      </form>

      <Section
        id="personal-orgs"
        title="Personal organizations from before"
        description="Created when every user got their own organization. Their owners can delete them; they stay until then."
      >
        {saved.personal_orgs.count === 0 ? (
          <p className="text-sm text-ink-2">None.</p>
        ) : (
          <>
            <p className="text-sm text-ink-2">
              {saved.personal_orgs.count} personal {saved.personal_orgs.count === 1 ? "organization" : "organizations"} from before.
              {saved.personal_orgs.count > saved.personal_orgs.items.length
                ? ` Showing ${saved.personal_orgs.items.length}.`
                : ""}
            </p>
            <DataTable caption="Personal organizations from before">
              <thead>
                <tr>
                  <Th>Organization</Th>
                  <Th>Owner</Th>
                </tr>
              </thead>
              <tbody>
                {saved.personal_orgs.items.map((o) => (
                  <tr key={o.org_id}>
                    <Td>
                      <div className="font-medium">{o.name}</div>
                      <div className="font-mono text-xs text-muted">{o.slug}</div>
                    </Td>
                    <Td className="[overflow-wrap:anywhere]">{o.owner_email}</Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </>
        )}
      </Section>
    </div>
  );
}
