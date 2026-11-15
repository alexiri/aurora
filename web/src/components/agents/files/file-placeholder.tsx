"use client";

import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ArrowLeftIcon, DownloadIcon } from "@/components/ui/icons";
import { baseName, downloadUrl, humanSize } from "@/lib/api/files";

/**
 * Opening a file (docs/14 §Files item 6): until the viewer lands (P5-FILES-3, which replaces this card with the editor,
 * the image preview and the "No preview" card) it is a card with the name, the size when known, and Download. Nothing
 * of the file is rendered.
 */
export function FilePlaceholder({ agent, path, size, onBack }: { agent: string; path: string; size?: number; onBack: () => void }) {
  return (
    <Card className="space-y-3 p-4">
      <div className="min-w-0">
        <h2 className="break-all font-display text-base font-semibold text-ink">{baseName(path)}</h2>
        <p className="break-all font-mono text-xs text-muted">{path}</p>
        {size !== undefined ? <p className="mt-1 text-sm text-ink-2">{humanSize(size)}</p> : null}
      </div>
      <p className="text-sm text-ink-2">The file viewer is not available yet. You can download the file.</p>
      <div className="flex flex-wrap gap-2">
        <a href={downloadUrl(agent, path)} download className={buttonVariants({ variant: "primary", size: "sm" })}>
          <DownloadIcon /> Download
        </a>
        <Button type="button" variant="outline" size="sm" onClick={onBack}>
          <ArrowLeftIcon /> Back to folder
        </Button>
      </div>
    </Card>
  );
}
