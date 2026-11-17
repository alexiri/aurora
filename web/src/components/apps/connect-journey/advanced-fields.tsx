"use client";

import { ScopeGroups } from "@/components/grants/scope-groups";
import { Field, inputClass } from "@/components/ui/field";
import { Segmented } from "@/components/ui/segmented";
import type { AppInstance, ConnectMethod, OAuthClient } from "@/lib/api/apps";

/**
 * The C-842 controls the journey keeps under **Advanced** (`14 §3`), extracted from the connect dialog so that the
 * journey and the Reconnect dialog use the same pieces.
 */

/** Instance-bound apps (D-74): the SaaS default host, or an instance an admin registered. */
export function InstanceField({
  id = "cm-instance",
  label,
  value,
  onChange,
  defaultHost,
  builtin,
  registered,
  error,
}: {
  id?: string;
  label: string;
  value: string;
  onChange: (v: string) => void;
  defaultHost: string | null;
  builtin: AppInstance[];
  registered: AppInstance[];
  error?: string;
}) {
  const none = builtin.length === 0 && registered.length === 0;
  return (
    <Field
      id={id}
      label={label}
      error={error}
      hint={
        none && !defaultHost
          ? "No self-hosted server is registered yet. Ask an organization admin to register one (Self-hosted apps)."
          : builtin.length > 0
            ? "Your token or sign-in is only sent to the host you choose here, so pick the one your organization lives on. A self-hosted server must be registered first (Self-hosted apps)."
            : "Your token or sign-in is only sent to the host you choose here."
      }
    >
      <select id={id} className={inputClass} value={value} onChange={(e) => onChange(e.target.value)}>
        {defaultHost ? <option value="">{`${defaultHost} (default)`}</option> : <option value="">Select…</option>}
        {/* (FIX-134, D-243) The package's own preset instances (a vendor's regions), then servers an admin registered. */}
        {builtin.map((i) => (
          <option key={i.id} value={i.id}>{`${i.label} (${i.host})`}</option>
        ))}
        {registered.length > 0 && builtin.length > 0 ? (
          <optgroup label="Registered servers">
            {registered.map((i) => (
              <option key={i.id} value={i.id}>{`${i.label} (${i.base_url})`}</option>
            ))}
          </optgroup>
        ) : (
          registered.map((i) => <option key={i.id} value={i.id}>{`${i.label} (${i.base_url})`}</option>)
        )}
      </select>
    </Field>
  );
}

/** The OAuth client to sign in with; "Automatic" uses the most specific enabled one. */
export function OAuthClientField({
  value,
  onChange,
  clients,
  id = "cm-client",
}: {
  value: string;
  onChange: (v: string) => void;
  clients: OAuthClient[];
  id?: string;
}) {
  return (
    <Field id={id} label="OAuth client" hint="Leave on automatic to use the most specific enabled client.">
      <select id={id} className={inputClass} value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Automatic</option>
        {clients.map((c) => (
          <option key={c.id} value={c.id}>{c.scope} client ({c.fields.client_id ?? c.client_key})</option>
        ))}
      </select>
    </Field>
  );
}

/** A sign-in method's requested scopes. */
export function RequestedScopes({
  method,
  selected,
  onChange,
  disabled,
}: {
  method: ConnectMethod;
  selected: string[];
  onChange: (s: string[]) => void;
  disabled?: boolean;
}) {
  return (
    <fieldset className="min-w-0 space-y-3">
      <legend className="mb-1 text-sm font-medium text-ink">Requested scopes</legend>
      <ScopeGroups scopes={method.scopes} selected={selected} onChange={onChange} disabled={disabled} />
    </fieldset>
  );
}

/** (FIX-136) A pasted token's scopes: detected by the method's read probes, or ticked by the person. */
export function TokenScopes({
  method,
  mode,
  onMode,
  selected,
  onChange,
  disabled,
}: {
  method: ConnectMethod;
  mode: "detect" | "choose";
  onMode: (m: "detect" | "choose") => void;
  selected: string[];
  onChange: (s: string[]) => void;
  disabled?: boolean;
}) {
  return (
    <fieldset className="min-w-0 space-y-3">
      <legend className="mb-1 text-sm font-medium text-ink">Which scopes did you give this token?</legend>
      <p className="text-xs text-muted">
        A pasted token does not tell Aurora what it may do. Aurora uses this only to hide tools the token cannot serve
        (they would fail upstream); it never gives the token more than it has. You can change it later on the connection.
      </p>
      {method.probe_scopes.length > 0 ? (
        <Segmented<"detect" | "choose">
          label="Token scopes"
          value={mode}
          onChange={onMode}
          options={[
            { value: "detect", label: "Detect read scopes" },
            { value: "choose", label: "I will tick them" },
          ]}
        />
      ) : null}
      {mode === "choose" || method.probe_scopes.length === 0 ? (
        <ScopeGroups scopes={method.scopes} selected={selected} onChange={onChange} disabled={disabled} />
      ) : (
        <p className="text-xs text-muted">
          Aurora checks each read scope with a harmless read request using this token. Write scopes are never checked:
          tick them yourself afterwards on the connection if the token has them.
        </p>
      )}
    </fieldset>
  );
}
