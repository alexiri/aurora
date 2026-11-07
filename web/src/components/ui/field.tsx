import { Children, cloneElement, isValidElement, type ReactElement, type ReactNode } from "react";

export const inputClass =
  "block w-full min-h-10 rounded-control border border-line-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-muted aria-[invalid=true]:border-danger-ink disabled:opacity-60 dark:bg-canvas";

/** Label, hint and error text linked to the control with `aria-describedby`; `aria-invalid` marks the error state. */
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
  const describedBy = error ? `${id}-error` : hint ? `${id}-hint` : undefined;
  const kids = Children.map(children, (child) => {
    if (!isValidElement(child)) return child;
    const el = child as ReactElement<Record<string, unknown>>;
    if (el.props.id !== id) return child;
    return cloneElement(el, {
      "aria-describedby": [el.props["aria-describedby"], describedBy].filter(Boolean).join(" ") || undefined,
      "aria-invalid": error ? true : el.props["aria-invalid"],
    });
  });
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="block text-sm font-medium text-ink">
        {label}
      </label>
      {kids}
      {hint && !error ? (
        <p id={`${id}-hint`} className="text-xs text-muted">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-xs text-danger-ink">
          {error}
        </p>
      ) : null}
    </div>
  );
}
