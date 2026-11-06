import type { ReactNode } from "react";

export const inputClass =
  "block w-full rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus-visible:outline-2 focus-visible:outline-offset-1";

export function Field({
  id,
  label,
  error,
  hint,
  children,
}: {
  id: string;
  label: string;
  error?: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="space-y-1">
      <label htmlFor={id} className="block text-sm font-medium">
        {label}
      </label>
      {children}
      {hint && !error ? <p className="text-xs text-neutral-500">{hint}</p> : null}
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-xs text-red-700">
          {error}
        </p>
      ) : null}
    </div>
  );
}
