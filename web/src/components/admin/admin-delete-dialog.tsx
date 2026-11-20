"use client";

import type { ReactNode } from "react";

import { TypeToConfirm } from "@/components/ui/type-to-confirm";

/**
 * The danger dialog of Admin › Organizations (P5-ADMDEL, docs/14 §Instance administration): names what goes and asks for
 * the **visible name** typed exactly (P5-REFINE-2; the delete button stays disabled until it matches). The first
 * confirm deletes, with no re-authentication step (phase 5, FIX-157, D-317). The API stays authoritative: the slug the
 * page sends with the delete is checked there too (400 `confirmation_mismatch`).
 */
export function AdminDeleteDialog({
  open,
  onOpenChange,
  title,
  consequences,
  noun,
  name,
  slug,
  run,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  consequences: ReactNode;
  /** "organization" or "workspace": the dialog's words. */
  noun: string;
  /** The visible name the person types. */
  name: string;
  /** What the API wants as its confirmation; sent by the page, never typed. */
  slug: string;
  /** Sends the delete with the slug; rejects with the API's error. */
  run: (confirm: string) => Promise<unknown>;
  onDone: () => void;
}) {
  return (
    <TypeToConfirm
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      consequence={<div className="space-y-2">{consequences}</div>}
      noun={noun}
      name={name}
      confirmLabel={`Delete ${noun}`}
      run={() => run(slug)}
      onDone={onDone}
    />
  );
}
