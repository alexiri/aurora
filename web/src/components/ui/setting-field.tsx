"use client";

import { Children, cloneElement, isValidElement, type ReactElement, type ReactNode } from "react";

import { Badge } from "./badge";
import { Button } from "./button";

export type SettingSource = "helm" | "dashboard" | "default";

const SOURCE_LABEL: Record<SettingSource, string> = { helm: "Helm", dashboard: "Dashboard", default: "Default" };

/** The id of a setting's control (`<key>-c`): give the control this `id` so the label links to it. */
export function settingControlId(key: string): string {
  return `${key}-c`;
}

/**
 * One instance setting (phase 5, SET-1, `14 §Page patterns`): label, hint, the control and a source
 * badge. **Locked** (`source` Helm): the effective value as read-only text and the note
 * "Configured in Helm (`<helm key>`)", no control. **Dashboard**: a ghost "Reset to default" action
 * naming the default. `id` is the setting key, so `#<key>` links scroll to it. The label, control
 * and source stack below 768 px.
 */
export function SettingField({
  id,
  label,
  hint,
  source,
  helmKey,
  lockedValue,
  defaultLabel,
  doc,
  onReset,
  error,
  children,
}: {
  /** The setting key (a dotted path): also the anchor. */
  id: string;
  label: string;
  hint?: string;
  source: SettingSource;
  helmKey?: string;
  /** What a locked setting shows instead of a control. */
  lockedValue?: ReactNode;
  /** The default, as text, for the reset action. */
  defaultLabel?: string;
  /** The title of the operator guide page that explains the setting (never a path). */
  doc?: string;
  onReset?: () => void;
  error?: string;
  /** The control, with `id={settingControlId(id)}`. */
  children?: ReactNode;
}) {
  const locked = source === "helm";
  const controlId = settingControlId(id);
  const describedBy = error ? `${controlId}-error` : hint ? `${controlId}-hint` : undefined;
  const kids = Children.map(children, (child) => {
    if (!isValidElement(child)) return child;
    const el = child as ReactElement<Record<string, unknown>>;
    if (el.props.id !== controlId) return child;
    return cloneElement(el, {
      "aria-describedby": [el.props["aria-describedby"], describedBy].filter(Boolean).join(" ") || undefined,
      "aria-invalid": error ? true : el.props["aria-invalid"],
    });
  });
  return (
    <div id={id} className="min-w-0 scroll-mt-24 space-y-2 border-b border-line pb-4 last:border-b-0 last:pb-0">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        {locked ? (
          <span className="text-sm font-medium text-ink">{label}</span>
        ) : (
          <label htmlFor={controlId} className="text-sm font-medium text-ink">
            {label}
          </label>
        )}
        {source === "default" && !locked && defaultLabel ? (
          // "Default: 15 minutes" — the default in words, not a bare "DEFAULT" badge (D-341).
          <span
            data-testid={`${id}-default`}
            className="inline-flex h-5 items-center rounded-full border border-line-strong px-2 text-label font-medium leading-none text-muted"
          >
            Default: {defaultLabel}
          </span>
        ) : (
          <Badge tone={locked ? "pending" : source === "dashboard" ? "accent" : "neutral"}>{SOURCE_LABEL[source]}</Badge>
        )}
      </div>
      {locked ? (
        <div className="space-y-1">
          <div className="break-words text-sm text-ink" data-testid={`${id}-locked-value`}>
            {lockedValue}
          </div>
          <p className="text-xs text-muted">
            Configured in Helm
            {helmKey ? (
              <>
                {" "}
                (<code className="font-mono">{helmKey}</code>)
              </>
            ) : null}
            . Remove it from your values to manage it here.
            {doc ? <> See “{doc}” in the operator guide.</> : null}
          </p>
        </div>
      ) : (
        <>
          {kids}
          {hint && !error ? (
            <p id={`${controlId}-hint`} className="text-xs text-muted">
              {hint}
            </p>
          ) : null}
          {error ? (
            <p id={`${controlId}-error`} role="alert" className="text-xs text-danger-ink">
              {error}
            </p>
          ) : null}
          {doc ? <p className="text-xs text-muted">More in “{doc}” in the operator guide.</p> : null}
          {source === "dashboard" && onReset ? (
            <Button type="button" variant="ghost" size="sm" onClick={onReset}>
              Reset to default{defaultLabel ? ` (${defaultLabel})` : ""}
            </Button>
          ) : null}
        </>
      )}
    </div>
  );
}
