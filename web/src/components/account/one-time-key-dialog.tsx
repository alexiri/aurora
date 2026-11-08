"use client";

import { useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";

/**
 * Shows a freshly created API key exactly once. The parent owns the value in
 * component state only and must clear it in `onClose`.
 */
export function OneTimeKeyDialog({
  secret,
  name,
  onClose,
  title = "Copy your new API key",
  description,
  children,
}: {
  secret: string | null;
  name?: string;
  onClose: () => void;
  title?: string;
  description?: string;
  children?: ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <Dialog
      open={secret !== null}
      onOpenChange={(o) => {
        if (!o) {
          setCopied(false);
          onClose();
        }
      }}
    >
      <DialogContent
        title={title}
        description={
          description ??
          `Key${name ? ` "${name}"` : ""} created. You won't see it again, so store it somewhere safe now.`
        }
      >
        {secret !== null ? (
          <div className="space-y-4">
            <code
              data-testid="secret-key"
              className="block break-all rounded-control border border-line bg-subtle p-3 font-mono text-sm text-ink"
            >
              {secret}
            </code>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="outline"
                onClick={() => {
                  void navigator.clipboard?.writeText(secret).then(() => setCopied(true));
                }}
              >
                Copy
              </Button>
              {copied ? (
                <span role="status" className="text-sm font-medium text-accent-ink">
                  Copied
                </span>
              ) : null}
              <Button
                className="ml-auto"
                onClick={() => {
                  setCopied(false);
                  onClose();
                }}
              >
                I have saved it
              </Button>
            </div>
            {children}
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
