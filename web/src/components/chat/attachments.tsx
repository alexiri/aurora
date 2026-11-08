"use client";

import { Badge } from "@/components/ui/badge";
import { DownloadIcon, FileIcon, XIcon } from "@/components/ui/icons";
import { attachmentUrl, canThumbnail, formatBytes, type Attachment } from "@/lib/api/attachments";
import type { PurgeReason } from "@/lib/chat/transcript";

const typeLabel = (contentType?: string): string => {
  const t = (contentType ?? "").split(";")[0].trim();
  if (!t) return "File";
  const sub = t.split("/")[1] ?? t;
  return sub.replace(/^x-/, "").replace(/^vnd\..*\./, "").toUpperCase().slice(0, 12);
};

/**
 * A file in a message: a type-icon tile, the name (the download link), the size and type; a raster image shows a
 * thumbnail instead of the tile. Content is never rendered inline (no HTML/SVG): the only embedded form is an
 * <img> of the download URL for raster images. A purged attachment is a muted chip (C-196), never a link.
 */
export function AttachmentCard({
  id,
  filename,
  contentType,
  size,
  purged,
}: {
  id: string;
  filename?: string;
  contentType?: string;
  size?: number;
  /** Set when the bytes are gone (C-196): a chip instead of a link, a thumbnail or a download. */
  purged?: PurgeReason;
}) {
  const url = attachmentUrl(id);
  const name = filename || "Attachment";
  if (purged) {
    return (
      <span data-testid="attachment-expired" className="inline-flex max-w-full flex-wrap items-center gap-2 text-sm text-muted">
        <span className="min-w-0 break-words">{name}</span>
        <Badge>{purged === "deleted" ? "Removed" : "Expired by retention"}</Badge>
      </span>
    );
  }
  const meta = [size !== undefined ? formatBytes(size) : "", typeLabel(contentType)].filter(Boolean).join(" · ");
  return (
    <span className="flex max-w-full flex-col gap-1.5">
      {canThumbnail(contentType) ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={url} alt="" className="max-h-60 max-w-60 rounded-control border border-line object-cover" />
      ) : null}
      <span className="flex min-w-0 items-center gap-2.5 rounded-control border border-line bg-surface px-2.5 py-2 text-sm text-ink dark:bg-canvas">
        <span aria-hidden="true" className="inline-flex size-9 shrink-0 items-center justify-center rounded-control bg-subtle text-ink-2">
          <FileIcon width={18} height={18} />
        </span>
        <span className="min-w-0 flex-1">
          <a className="block truncate font-medium text-link underline" href={url} download={filename || undefined}>
            {name}
          </a>
          <span className="block truncate text-xs text-muted">{meta}</span>
        </span>
        <DownloadIcon aria-hidden="true" className="shrink-0 text-ink-2" />
      </span>
    </span>
  );
}

/** A composer chip above the text field: thumbnail or icon, name, size and Remove. */
export function ComposerChip({ attachment, onRemove }: { attachment: Attachment; onRemove: () => void }) {
  return (
    <li className="flex min-h-11 max-w-full items-center gap-2 rounded-control border border-line-strong bg-surface px-2 py-1 text-xs text-ink-2 dark:bg-canvas">
      {canThumbnail(attachment.content_type) ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={attachmentUrl(attachment.id)} alt="" className="size-8 shrink-0 rounded object-cover" />
      ) : (
        <span aria-hidden="true" className="inline-flex size-8 shrink-0 items-center justify-center rounded bg-subtle">
          <FileIcon width={16} height={16} />
        </span>
      )}
      <span className="min-w-0 truncate">
        {attachment.filename} ({formatBytes(attachment.size_bytes)})
      </span>
      <button
        type="button"
        className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink hover:bg-subtle hover:text-danger-ink md:size-8"
        aria-label={`Remove ${attachment.filename}`}
        onClick={onRemove}
      >
        <XIcon width={14} height={14} />
      </button>
    </li>
  );
}

/** A chip for an upload in progress or one that failed (the reason is `uploadErrorMessage`). */
export function UploadChip({ name, error }: { name: string; error?: string }) {
  return error ? (
    <div role="alert" className="flex min-h-9 max-w-full items-center gap-2 rounded-control border border-danger-ink/30 bg-danger-soft px-2.5 py-1 text-xs text-danger-ink">
      <span className="min-w-0 break-words">{error}</span>
    </div>
  ) : (
    <div role="status" className="flex min-h-9 max-w-full items-center gap-2 rounded-control border border-line-strong bg-surface px-2.5 py-1 text-xs text-muted dark:bg-canvas">
      <span aria-hidden="true" className="size-3.5 shrink-0 animate-spin rounded-full border-2 border-line-strong border-t-accent motion-reduce:animate-none" />
      <span className="min-w-0 truncate">Uploading {name}…</span>
    </div>
  );
}
