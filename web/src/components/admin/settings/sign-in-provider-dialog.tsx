"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/form";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";
import { Notice } from "@/components/ui/notice";
import { StatusDot } from "@/components/ui/status-dot";
import { errorMessage } from "@/lib/api/errors";
import { useTestProvider, type ProviderTestResult } from "@/lib/api/settings";

type Entry = Record<string, unknown>;

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const lines = (v: unknown): string => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").join("\n") : "");
const split = (t: string): string[] =>
  t
    .split("\n")
    .map((x) => x.trim())
    .filter(Boolean);
const obj = (v: unknown): Record<string, unknown> => (typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

/** How the dialog treats the client secret: keep the stored one, take the environment's (Helm-mirrored), or a new one. */
type SecretMode = "keep" | "env" | "replace";

interface Mapping {
  value: string;
  instance_admin: boolean;
  org_slug: string;
  org_role: string;
  workspaces: string;
}

interface Draft {
  id: string;
  display_name: string;
  issuer: string;
  client_id: string;
  secretMode: SecretMode;
  secret: string;
  scopes: string;
  allowed_email_domains: string;
  trusted_email_domains: string;
  link_existing_users: "default" | "yes" | "no";
  role_claim: string;
  role_claim_source: string;
  require_role: boolean;
  mappings: Mapping[];
  group_claim: string;
  group_claim_source: string;
  jit: boolean;
  jit_org: string;
  jit_role: string;
  jit_workspaces: string;
  jit_workspace_role: string;
}

function toMapping(m: unknown): Mapping {
  const o = obj(m);
  const org = obj(o.org);
  const ws = Array.isArray(o.workspaces) ? o.workspaces.map((w) => `${str(obj(w).slug)}:${str(obj(w).role) || "member"}`) : [];
  return {
    value: str(o.value),
    instance_admin: o.instance_admin === true,
    org_slug: str(org.slug),
    org_role: str(org.role) || "member",
    workspaces: ws.join("\n"),
  };
}

function toDraft(e: Entry | null): Draft {
  const jit = obj(e?.jit);
  const link = e?.link_existing_users;
  const hasSealed = typeof e?.client_secret === "object" && e?.client_secret !== null;
  const hasEnv = str(e?.client_secret_env) !== "";
  return {
    id: str(e?.id),
    display_name: str(e?.display_name),
    issuer: str(e?.issuer) || str(e?.issuer_url),
    client_id: str(e?.client_id),
    secretMode: e === null ? "replace" : hasSealed ? "keep" : hasEnv ? "env" : "replace",
    secret: "",
    scopes: lines(e?.scopes),
    allowed_email_domains: lines(e?.allowed_email_domains),
    trusted_email_domains: lines(e?.trusted_email_domains),
    link_existing_users: link === true ? "yes" : link === false ? "no" : "default",
    role_claim: str(e?.role_claim),
    role_claim_source: str(e?.role_claim_source) || "id_token",
    require_role: e?.require_role === true,
    mappings: Array.isArray(e?.role_mappings) ? e.role_mappings.map(toMapping) : [],
    group_claim: str(e?.group_claim),
    group_claim_source: str(e?.group_claim_source) || "id_token",
    jit: e?.jit !== undefined && e?.jit !== null,
    jit_org: str(jit.org_slug),
    jit_role: str(jit.role) || "member",
    jit_workspaces: lines(jit.workspace_slugs),
    jit_workspace_role: str(jit.workspace_role) || "member",
  };
}

function wsList(text: string): { slug: string; role: string }[] {
  return split(text).map((l) => {
    const [slug, role] = l.split(":");
    return { slug: (slug ?? "").trim(), role: (role ?? "member").trim() || "member" };
  });
}

/** The entry the draft describes: every key the dialog does not edit is kept as it was. */
function toEntry(d: Draft, base: Entry | null, withSecret: boolean): Entry {
  const out: Entry = { ...(base ?? {}) };
  const put = (k: string, v: unknown) => {
    if (v === undefined || v === "" || v === null || (Array.isArray(v) && v.length === 0)) delete out[k];
    else out[k] = v;
  };
  out.id = d.id.trim();
  delete out.issuer_url;
  put("display_name", d.display_name.trim());
  out.issuer = d.issuer.trim();
  out.client_id = d.client_id.trim();
  // Never a reference to an environment variable or a file from here: only a Helm-mirrored entry keeps its own.
  if (d.secretMode === "replace") {
    delete out.client_secret_env;
    if (withSecret) out.client_secret = { $secret: d.secret };
    else delete out.client_secret;
  } else if (!withSecret) {
    delete out.client_secret;
  }
  put("scopes", split(d.scopes));
  put("allowed_email_domains", split(d.allowed_email_domains));
  put("trusted_email_domains", split(d.trusted_email_domains));
  if (d.link_existing_users === "default") delete out.link_existing_users;
  else out.link_existing_users = d.link_existing_users === "yes";
  put("role_claim", d.role_claim.trim());
  put("role_claim_source", d.role_claim.trim() ? d.role_claim_source : "");
  put("require_role", d.require_role ? true : "");
  put(
    "role_mappings",
    d.mappings
      .filter((m) => m.value.trim())
      .map((m) => {
        const r: Entry = { value: m.value.trim() };
        if (m.instance_admin) r.instance_admin = true;
        if (m.org_slug.trim()) {
          r.org = { slug: m.org_slug.trim(), role: m.org_role };
          const ws = wsList(m.workspaces).filter((w) => w.slug);
          if (ws.length) r.workspaces = ws;
        }
        return r;
      }),
  );
  put("group_claim", d.group_claim.trim());
  put("group_claim_source", d.group_claim.trim() ? d.group_claim_source : "");
  if (d.jit) {
    const j: Entry = { org_slug: d.jit_org.trim(), role: d.jit_role, workspace_role: d.jit_workspace_role };
    const ws = split(d.jit_workspaces);
    if (ws.length) j.workspace_slugs = ws;
    out.jit = j;
  } else {
    delete out.jit;
  }
  return out;
}

const REASON: Record<string, string> = {
  issuer_invalid: "That is not a valid issuer address.",
  issuer_not_https: "The issuer must be an https address.",
  destination_refused: "This address is private, loopback or a metadata address. An operator can open it with gateway.allowed_private.",
  issuer_mismatch: "The issuer in the discovery document is not the one you entered.",
  not_oidc: "The address did not answer with an OpenID Connect discovery document.",
  http_error: "The address answered with an error.",
  unreachable: "The address could not be reached from the API.",
};

function Discovery({ r }: { r: ProviderTestResult }) {
  return (
    <div role="status" aria-label="Discovery result" className="space-y-1 rounded-control border border-line p-3">
      <StatusDot tone={r.ok ? "accent" : "danger"}>
        {r.ok ? "Discovery succeeded." : `Discovery failed${r.error_code && REASON[r.error_code] ? `: ${REASON[r.error_code]}` : "."}`}
      </StatusDot>
      {r.ok ? (
        <dl className="space-y-1 text-xs text-ink-2">
          {r.issuer ? <div className="break-all">Issuer: {r.issuer}</div> : null}
          {r.authorization_endpoint ? <div className="break-all">Authorization: {r.authorization_endpoint}</div> : null}
          {r.token_endpoint ? <div className="break-all">Token: {r.token_endpoint}</div> : null}
          {r.jwks_uri ? <div className="break-all">Keys: {r.jwks_uri}</div> : null}
        </dl>
      ) : null}
    </div>
  );
}

/** A Helm-defined provider: read-only, with where it is configured. No secret is ever shown. */
export function HelmProviderDialog({ entry, helmKey, onClose }: { entry: Entry; helmKey: string; onClose: () => void }) {
  const rows = Object.entries(entry).filter(([k, v]) => v !== null && v !== undefined && k !== "client_secret");
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent title={`Provider ${str(entry.id)}`} description="Managed by Helm." size="xl">
        <div className="space-y-4">
          <Notice tone="pending" role="status">
            Configured in Helm (<code className="font-mono">{helmKey}</code>). Change it in the values file. It stays available
            even if the dashboard providers are misconfigured.
          </Notice>
          <dl className="space-y-2 text-sm">
            {rows.map(([k, v]) => (
              <div key={k} className="flex flex-col gap-1 sm:flex-row sm:gap-3">
                <dt className="font-medium text-ink sm:w-44 sm:shrink-0">{k}</dt>
                <dd className="min-w-0 flex-1 break-words font-mono text-xs text-ink-2">{typeof v === "string" ? v : JSON.stringify(v)}</dd>
              </div>
            ))}
          </dl>
          <div className="flex justify-end">
            <DialogClose asChild>
              <Button type="button" variant="outline">
                Close
              </Button>
            </DialogClose>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function Fieldset({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <fieldset className="space-y-3 rounded-control border border-line p-3">
      <legend className="px-1 text-sm font-medium text-ink">{title}</legend>
      {children}
    </fieldset>
  );
}

/** Add or edit a dashboard provider (SET-4, C-780): basics, e-mail, roles, groups, JIT and a discovery test. */
export function ProviderDialog({
  existing,
  helmIds,
  otherIds,
  errors,
  signedInWith,
  onSave,
  onClose,
}: {
  existing: Entry | null;
  helmIds: string[];
  otherIds: string[];
  /** Field errors of a failed save for this entry, by field (`issuer`, `role_mappings/0/value`, …). */
  errors: Record<string, string>;
  /** The provider id the admin signed in with: editing it needs care. */
  signedInWith?: string | null;
  onSave: (e: Entry) => void;
  onClose: () => void;
}) {
  const [d, setD] = useState<Draft>(() => toDraft(existing));
  const [result, setResult] = useState<ProviderTestResult | null>(null);
  const test = useTestProvider();
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => {
    setResult(null);
    setD((x) => ({ ...x, [k]: v }));
  };
  const setMapping = (i: number, patch: Partial<Mapping>) => setD((x) => ({ ...x, mappings: x.mappings.map((m, j) => (j === i ? { ...m, ...patch } : m)) }));

  const id = d.id.trim();
  const idError =
    errors.id ??
    (id && helmIds.includes(id)
      ? `\`${id}\` is defined in Helm; choose another id`
      : id && otherIds.includes(id)
        ? `\`${id}\` is already a provider`
        : id && !/^[a-z0-9][a-z0-9_-]*$/.test(id)
          ? "Lowercase letters, digits, `_` or `-`."
          : undefined);
  const baseIssuer = str(existing?.issuer) || str(existing?.issuer_url);
  const retargets = existing !== null && d.secretMode !== "replace" && (d.issuer.trim() !== baseIssuer || d.client_id.trim() !== str(existing.client_id));
  const missing =
    !id ||
    !d.issuer.trim() ||
    !d.client_id.trim() ||
    (d.secretMode === "replace" && !d.secret) ||
    (d.jit && !d.jit_org.trim()) ||
    retargets;
  const grantsAdmin = d.mappings.some((m) => m.instance_admin);

  function submit(e: FormEvent) {
    e.preventDefault();
    // The dialog sits inside the page's form in the React tree: its submit must not save the page.
    e.stopPropagation();
    if (idError || missing) return;
    onSave(toEntry(d, existing, true));
  }

  const secretStatus =
    d.secretMode === "keep"
      ? "A client secret is stored (sealed)."
      : d.secretMode === "env"
        ? `Read from the environment variable ${str(existing?.client_secret_env)} (set in Helm).`
        : null;

  return (
    <FormDialog
      open
      size="2xl"
      onOpenChange={(o) => !o && onClose()}
      title={existing ? `Edit ${str(existing.id)}` : "Add an identity provider"}
      description="Any OpenID Connect provider. The client secret is sealed and never shown again."
      submitLabel="Done"
      submitDisabled={Boolean(idError) || missing}
      onSubmit={submit}
    >
      {existing && signedInWith === str(existing.id) ? (
        <Notice tone="pending" role="status">
          You are signed in through this provider. A change that would leave you without a way to sign in is refused when you save.
        </Notice>
      ) : null}

      <Fieldset title="Basics">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="op-id" label="Provider id" error={idError} hint="Appears in the sign-in address. It cannot be changed later.">
            <input id="op-id" required disabled={existing !== null} className={inputClass} value={d.id} onChange={(e) => set("id", e.target.value)} />
          </Field>
          <Field id="op-name" label="Display name" error={errors.display_name} hint="The label on the sign-in button.">
            <input id="op-name" className={inputClass} value={d.display_name} onChange={(e) => set("display_name", e.target.value)} />
          </Field>
        </div>
        <Field id="op-issuer" label="Issuer" error={errors.issuer} hint="https://sso.example.org/realms/main. Aurora reads the provider's discovery document from here.">
          <input id="op-issuer" required className={inputClass} value={d.issuer} onChange={(e) => set("issuer", e.target.value)} />
        </Field>
        <Field id="op-client" label="Client id" error={errors.client_id}>
          <input id="op-client" required className={inputClass} value={d.client_id} onChange={(e) => set("client_id", e.target.value)} />
        </Field>
        <Field
          id="op-secret"
          label="Client secret"
          error={errors.client_secret ?? errors.client_secret_env}
          hint="Write-only. Entered once and sealed; it appears in no response, export or log."
        >
          <div id="op-secret" className="space-y-2">
            {secretStatus ? <p className="text-sm text-ink-2">{secretStatus}</p> : null}
            {d.secretMode === "replace" ? (
              <SecretInput
                aria-label="Client secret: new value"
                revealLabel="client secret"
                value={d.secret}
                onChange={(v) => set("secret", v)}
              />
            ) : (
              <Button type="button" size="sm" variant="outline" onClick={() => setD((x) => ({ ...x, secretMode: "replace", secret: "" }))}>
                Replace…
              </Button>
            )}
            {d.secretMode === "replace" && existing !== null && (str(existing.client_secret_env) || existing.client_secret) ? (
              <Button type="button" size="sm" variant="ghost" onClick={() => setD((x) => ({ ...x, secretMode: str(existing.client_secret_env) ? "env" : "keep", secret: "" }))}>
                Keep the current secret
              </Button>
            ) : null}
          </div>
        </Field>
        {retargets ? (
          <Notice tone="danger" role="alert">
            Changing the issuer or client id needs the client secret entered again: a stored secret is never carried to another
            issuer or client.
          </Notice>
        ) : null}
        <Field id="op-scopes" label="Scopes" error={errors.scopes} hint="One per line. Empty uses openid, email and profile.">
          <textarea id="op-scopes" rows={2} spellCheck={false} className={`${inputClass} font-mono`} value={d.scopes} onChange={(e) => set("scopes", e.target.value)} />
        </Field>
      </Fieldset>

      <Fieldset title="E-mail">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="op-allowed" label="Allowed e-mail domains" error={errors.allowed_email_domains} hint="One per line. Empty allows any verified address.">
            <textarea id="op-allowed" rows={2} spellCheck={false} className={`${inputClass} font-mono`} value={d.allowed_email_domains} onChange={(e) => set("allowed_email_domains", e.target.value)} />
          </Field>
          <Field id="op-trusted" label="Trusted e-mail domains" error={errors.trusted_email_domains} hint="One per line. Domains this provider is authoritative for, for providers that never send email_verified. Loosens sign-in.">
            <textarea id="op-trusted" rows={2} spellCheck={false} className={`${inputClass} font-mono`} value={d.trusted_email_domains} onChange={(e) => set("trusted_email_domains", e.target.value)} />
          </Field>
        </div>
        <Field id="op-link" label="Link existing users" hint="Whether a first sign-in may join an account created through another provider with the same e-mail.">
          <select id="op-link" className={inputClass} value={d.link_existing_users} onChange={(e) => set("link_existing_users", e.target.value as Draft["link_existing_users"])}>
            <option value="default">Default (yes, unless trusted domains are used)</option>
            <option value="yes">Yes</option>
            <option value="no">No</option>
          </select>
        </Field>
      </Fieldset>

      <Fieldset title="Roles">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="op-role-claim" label="Role claim" error={errors.role_claim} hint="Dot path into the token, e.g. cern_roles. Empty = no role mapping.">
            <input id="op-role-claim" className={inputClass} value={d.role_claim} onChange={(e) => set("role_claim", e.target.value)} />
          </Field>
          <Field id="op-role-source" label="Role claim source">
            <select id="op-role-source" className={inputClass} value={d.role_claim_source} onChange={(e) => set("role_claim_source", e.target.value)}>
              <option value="id_token">ID token</option>
              <option value="userinfo">Userinfo</option>
            </select>
          </Field>
        </div>
        <CheckboxField id="op-require-role" label="Require a mapped role" hint="A sign-in whose claim maps to nothing is refused." checked={d.require_role} onChange={(v) => set("require_role", v)} />
        <div className="space-y-2">
          <p className="text-sm font-medium text-ink">Role mappings</p>
          {d.mappings.length === 0 ? <p className="text-xs text-muted">No mapping.</p> : null}
          {d.mappings.map((m, i) => (
            <div key={i} className="space-y-2 rounded-control border border-line p-3">
              <div className="grid gap-3 sm:grid-cols-2">
                <Field id={`op-map-${i}-value`} label={`Mapping ${i + 1}: claim value`} error={errors[`role_mappings/${i}/value`]}>
                  <input id={`op-map-${i}-value`} className={inputClass} value={m.value} onChange={(e) => setMapping(i, { value: e.target.value })} />
                </Field>
                <div className="flex items-end pb-2">
                  <CheckboxField id={`op-map-${i}-admin`} label="Instance administrator" checked={m.instance_admin} onChange={(v) => setMapping(i, { instance_admin: v })} />
                </div>
              </div>
              <div className="grid gap-3 sm:grid-cols-3">
                <Field id={`op-map-${i}-org`} label={`Mapping ${i + 1}: organization slug`} error={errors[`role_mappings/${i}/org`]}>
                  <input id={`op-map-${i}-org`} className={inputClass} value={m.org_slug} onChange={(e) => setMapping(i, { org_slug: e.target.value })} />
                </Field>
                <Field id={`op-map-${i}-orole`} label={`Mapping ${i + 1}: organization role`}>
                  <select id={`op-map-${i}-orole`} className={inputClass} value={m.org_role} onChange={(e) => setMapping(i, { org_role: e.target.value })}>
                    <option value="member">member</option>
                    <option value="admin">admin</option>
                    <option value="owner">owner</option>
                  </select>
                </Field>
                <Field id={`op-map-${i}-ws`} label={`Mapping ${i + 1}: workspaces`} hint="slug:role per line (owner, approver, member).">
                  <textarea id={`op-map-${i}-ws`} rows={2} spellCheck={false} className={`${inputClass} font-mono`} value={m.workspaces} onChange={(e) => setMapping(i, { workspaces: e.target.value })} />
                </Field>
              </div>
              <Button type="button" size="sm" variant="danger" aria-label={`Remove mapping ${i + 1}`} onClick={() => set("mappings", d.mappings.filter((_, j) => j !== i))}>
                Remove mapping
              </Button>
            </div>
          ))}
          <Button type="button" size="sm" variant="outline" onClick={() => set("mappings", [...d.mappings, { value: "", instance_admin: false, org_slug: "", org_role: "member", workspaces: "" }])}>
            Add mapping
          </Button>
          {grantsAdmin ? (
            <Notice tone="danger" role="status">
              A mapping to instance administrator gives everyone who carries that claim value full control of this instance.
            </Notice>
          ) : null}
        </div>
      </Fieldset>

      <Fieldset title="Groups">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="op-group-claim" label="Group claim" error={errors.group_claim} hint="Dot path into the token. Empty = this provider feeds no IdP-linked group.">
            <input id="op-group-claim" className={inputClass} value={d.group_claim} onChange={(e) => set("group_claim", e.target.value)} />
          </Field>
          <Field id="op-group-source" label="Group claim source">
            <select id="op-group-source" className={inputClass} value={d.group_claim_source} onChange={(e) => set("group_claim_source", e.target.value)}>
              <option value="id_token">ID token</option>
              <option value="userinfo">Userinfo</option>
            </select>
          </Field>
        </div>
      </Fieldset>

      <Fieldset title="Just-in-time sign-up">
        <CheckboxField id="op-jit" label="Create accounts on first sign-in into an organization" checked={d.jit} onChange={(v) => set("jit", v)} />
        {d.jit ? (
          <div className="grid gap-3 sm:grid-cols-2">
            <Field id="op-jit-org" label="Organization slug" error={errors["jit/org_slug"] ?? errors.jit}>
              <input id="op-jit-org" className={inputClass} value={d.jit_org} onChange={(e) => set("jit_org", e.target.value)} />
            </Field>
            <Field id="op-jit-role" label="Organization role">
              <select id="op-jit-role" className={inputClass} value={d.jit_role} onChange={(e) => set("jit_role", e.target.value)}>
                <option value="member">member</option>
                <option value="admin">admin</option>
              </select>
            </Field>
            <Field id="op-jit-ws" label="Workspaces" hint="Workspace slugs, one per line.">
              <textarea id="op-jit-ws" rows={2} spellCheck={false} className={`${inputClass} font-mono`} value={d.jit_workspaces} onChange={(e) => set("jit_workspaces", e.target.value)} />
            </Field>
            <Field id="op-jit-wrole" label="Workspace role">
              <select id="op-jit-wrole" className={inputClass} value={d.jit_workspace_role} onChange={(e) => set("jit_workspace_role", e.target.value)}>
                <option value="member">member</option>
                <option value="approver">approver</option>
              </select>
            </Field>
          </div>
        ) : null}
      </Fieldset>

      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="button"
          variant="outline"
          disabled={test.isPending || !d.issuer.trim()}
          onClick={() => {
            setResult(null);
            test.mutate(toEntry(d, existing, false), { onSuccess: setResult });
          }}
        >
          {test.isPending ? "Testing…" : "Test discovery"}
        </Button>
        <span className="text-xs text-muted">Fetches the provider&apos;s discovery document. Nothing is saved and no secret is sent.</span>
      </div>
      {test.isError ? (
        <Notice tone="danger" role="alert">
          {errorMessage(test.error)}
        </Notice>
      ) : null}
      {result ? <Discovery r={result} /> : null}
    </FormDialog>
  );
}
