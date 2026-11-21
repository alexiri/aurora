import { Children, cloneElement, isValidElement, type ReactElement, type ReactNode } from "react";

export const inputClass =
  "block w-full min-h-10 rounded-control border border-line-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-muted aria-[invalid=true]:border-danger-ink disabled:opacity-60 dark:bg-canvas";

/**
 * (P5-UX-1) The label of a field that may be left empty: "(optional)" exactly once, however the label arrived (a
 * manifest's own label may already end in it).
 */
export function fieldLabel(label: string, optional: boolean): string {
  const bare = label.replace(/\s*\(optional\)\s*$/i, "").trim();
  return optional ? `${bare} (optional)` : bare;
}

/** Label, hint and error text linked to the control with `aria-describedby`; `aria-invalid` marks the error state. */
export function Field({
  id,
  label,
  error,
  hint,
  children,
  className,
}: {
  /** The field's width where it shares a line (`w-80`, `max-w-xs`…); full width when absent. */
  className?: string;
  id: string;
  /** Usually text; a node when the label carries a marker (a "Set by …" badge). */
  label: ReactNode;
  error?: string;
  hint?: ReactNode;
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
    <div className={className ? `space-y-1.5 ${className}` : "space-y-1.5"}>
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
