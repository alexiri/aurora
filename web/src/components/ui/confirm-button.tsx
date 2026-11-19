"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";

/**
 * An in-place two-step destructive action: the danger button (`label`) turns into `confirmLabel` and a Cancel. For the
 * lighter case where a whole dialog would be too much; use `ConfirmDialog` when the consequence needs explaining.
 */
export function ConfirmButton({
  label = "Delete",
  confirmLabel = "Confirm delete",
  onConfirm,
  pending,
  size,
  type,
  cancelVariant = "outline",
}: {
  label?: string;
  confirmLabel?: string;
  onConfirm: () => void;
  pending?: boolean;
  size?: "sm";
  type?: "button";
  cancelVariant?: "outline" | "ghost";
}) {
  const [confirming, setConfirming] = useState(false);
  if (!confirming) {
    return (
      <Button type={type} variant="danger" size={size} onClick={() => setConfirming(true)}>
        {label}
      </Button>
    );
  }
  return (
    <>
      <Button type={type} variant="danger" size={size} disabled={pending} onClick={onConfirm}>
        {confirmLabel}
      </Button>
      <Button type={type} variant={cancelVariant} size={size} onClick={() => setConfirming(false)}>
        Cancel
      </Button>
    </>
  );
}
