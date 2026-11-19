"use client";

import Link from "next/link";

import { LockPill } from "@/components/ui/lock-pill";
import { inputClass } from "@/components/ui/field";
import { Select } from "@/components/ui/select";
import { useMe } from "@/lib/api/queries";
import { adminRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/**
 * "Enforced by instance". With `setting` (the registry key, C-783) instance admins get a link to
 * the section's page (AI models › Rules) at the key; everyone else sees plain text.
 */
export function Locked({ setting }: { setting?: string }) {
  const me = useMe();
  const badge = (
    <LockPill owner="Set by Administration" className="ml-2 align-middle" />
  );
  if (!setting || me.data?.user.is_instance_admin !== true) return badge;
  return (
    <Link href={adminRoutes.governanceSetting(setting)} className="no-underline" aria-label={`Set by Administration: open the ${setting} setting`}>
      {badge}
    </Link>
  );
}

export function Errors({ messages }: { messages: string[] }) {
  if (messages.length === 0) return null;
  return (
    <ul role="alert" className="text-xs text-danger-ink">
      {messages.map((m, i) => (
        <li key={i}>{m}</li>
      ))}
    </ul>
  );
}

export function PostureSelect<T extends string>({
  label, value, options, locked, readOnly, onChange, setting,
}: {
  label: string; value: T; options: { value: T; label: string }[]; locked: boolean; readOnly: boolean; onChange: (v: T) => void;
  /** The instance setting that decides when `locked` (C-783). */
  setting?: string;
}) {
  return (
    <label className="block text-sm font-medium text-ink">
      <span>{label}{locked ? <Locked setting={setting} /> : null}</span>
      <Select value={value} disabled={locked || readOnly} className="mt-1.5 max-w-lg" onChange={(v) => onChange(v as T)} options={options.map((o) => ({ value: o.value, label: o.label }))} />
    </label>
  );
}

export function TextArea({ id, label, hint, value, locked, readOnly, onChange, errors }: {
  id: string; label: string; hint?: string; value: string; locked?: boolean; readOnly: boolean; onChange: (v: string) => void; errors: string[];
}) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="block text-sm font-medium text-ink">
        {label}{locked ? <Locked /> : null}
      </label>
      <textarea id={id} rows={3} className={cn(inputClass, "font-mono")} value={value} disabled={readOnly || locked} onChange={(e) => onChange(e.target.value)} aria-invalid={errors.length > 0} />
      {hint ? <p className="text-xs text-muted">{hint}</p> : null}
      <Errors messages={errors} />
    </div>
  );
}
