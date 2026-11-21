"use client";

import { Children, cloneElement, isValidElement, type ReactElement, type ReactNode } from "react";

import { cn } from "@/lib/utils";

import { Badge } from "./badge";
import { Button } from "./button";
import { BookIcon } from "./icons";
import { LockPill } from "./lock-pill";

export type SettingSource = "helm" | "dashboard" | "default";

/** The id of a setting's control (`<key>-c`): give the control this `id` so the label links to it. */
export function settingControlId(key: string): string {
  return `${key}-c`;
}

/**
 * (P5-POLISH-2, D-392, E7) A card of settings: a title, an optional one-line sentence and **one** guide reference in
 * its header, then the rows. It replaces the "More in … in the operator guide." line under every field. `guide` is the
 * title of the operator guide page (never a path).
 */
export function SettingsCard({
  title,
  description,
  guide,
  children,
  className,
}: {
  title: string;
  description?: ReactNode;
  guide?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("min-w-0 rounded-card border border-line bg-surface", className)}>
      <header className="flex flex-wrap items-start justify-between gap-x-4 gap-y-1 border-b border-line px-4 py-3 sm:px-5">
        <div className="min-w-0">
          <h3 className="font-display text-base font-semibold text-ink">{title}</h3>
          {description ? <p className="mt-0.5 text-sm text-ink-2">{description}</p> : null}
        </div>
        {guide ? (
          <p className="inline-flex shrink-0 items-center gap-1 text-xs text-muted">
            <BookIcon aria-hidden="true" className="size-3.5" />
            Guide: “{guide}”
          </p>
        ) : null}
      </header>
      <div className="divide-y divide-line px-4 sm:px-5">{children}</div>
    </section>
  );
}

/**
 * One instance setting (phase 5, SET-1, D-392, `14 §Page patterns`) as a two-column row: on the left the label and its
 * help, on the right the control. Rows stack below 640 px. **Locked** (`source` Helm): the right column is a neutral
 * "Set in Helm" lock pill and the effective value, read-only. **Dashboard**: a "Dashboard" badge and a ghost "Reset to
 * default" action naming the default. A boolean control is a `Switch`. `id` is the setting key, so `#<key>` links
 * scroll to it. `doc` (the guide page title) is kept on the row as `data-doc`;
 * the visible guide reference is the card header's.
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
  /** The title of the operator guide page that explains the setting (never a path); the card header carries it. */
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
    <div id={id} data-doc={doc} className="grid min-w-0 scroll-mt-24 gap-x-6 gap-y-2 py-4 sm:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
      <div className="min-w-0 space-y-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          {locked ? (
            <span className="text-sm font-medium text-ink">{label}</span>
          ) : (
            <label htmlFor={controlId} className="text-sm font-medium text-ink">
              {label}
            </label>
          )}
          {source === "dashboard" ? <Badge tone="accent">Dashboard</Badge> : null}
        </div>
        {hint && !error ? (
          <p id={`${controlId}-hint`} className="text-xs text-muted">
            {hint}
          </p>
        ) : null}
        {source === "default" && !locked && defaultLabel ? (
          // "Default: 15 minutes" in the help text, in words (D-341); a row carries no chip for it (F2, P5-POLISH-11).
          <p data-testid={`${id}-default`} className="text-xs text-muted">
            Default: {defaultLabel}
          </p>
        ) : null}
      </div>
      <div className="min-w-0 space-y-2">
        {locked ? (
          <div className="space-y-1">
            <LockPill owner="Set by the installation" settingKey={id} />
            <div className="break-words text-sm text-ink" data-testid={`${id}-locked-value`}>
              {lockedValue}
            </div>
            {helmKey ? (
              <p className="text-xs text-muted">
                Set in your Helm values: <code className="font-mono">{helmKey}</code>
              </p>
            ) : null}
          </div>
        ) : (
          <>
            {kids}
            {error ? (
              <p id={`${controlId}-error`} role="alert" className="text-xs text-danger-ink">
                {error}
              </p>
            ) : null}
            {source === "dashboard" && onReset ? (
              // A long default ("Follows what your deployment provides") wraps at 390 px instead of widening the page.
              <Button type="button" variant="ghost" size="sm" className="h-auto max-w-full whitespace-normal py-1.5 text-left" onClick={onReset}>
                Reset to default{defaultLabel ? ` (${defaultLabel})` : ""}
              </Button>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}
