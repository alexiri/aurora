"use client";

import { useRef, useState, type DragEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** True when the drag carries files (not selected text or a link dragged out of the page). */
const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");

/**
 * Wraps a pane so files dropped anywhere on it are handed to `onFiles` (P5-CHAT-3, docs/14 "Files"): while files are
 * dragged over it an overlay says "Drop files to attach". Attach and paste do the same, so drop is never the only
 * way. The overlay ignores pointer events (the drop lands on the wrapper); a counter handles nested enter/leave.
 */
export function DropZone({
  onFiles,
  onDataTransfer,
  label = "Drop files to attach",
  icon,
  disabled = false,
  className,
  children,
}: {
  onFiles?: (files: File[]) => void;
  /**
   * Gets the whole `DataTransfer` instead of the flat file list (P5-FILES-2): a dropped folder is only visible through
   * `items[i].webkitGetAsEntry()`, which must be called synchronously inside the drop event.
   */
  onDataTransfer?: (data: DataTransfer) => void;
  /** The overlay's words; the Files page names the folder ("Drop to upload to /workspace"). */
  label?: string;
  /** Shown in an `accent-soft` circle above the label (the Files page's `UploadIcon`). */
  icon?: ReactNode;
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const [over, setOver] = useState(false);
  const depth = useRef(0);
  return (
    <div
      className={cn("relative", className)}
      data-dropping={over || undefined}
      onDragEnter={(e) => {
        if (disabled || !hasFiles(e)) return;
        depth.current += 1;
        setOver(true);
      }}
      onDragOver={(e) => {
        if (disabled || !hasFiles(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={(e) => {
        if (disabled || !hasFiles(e)) return;
        depth.current = Math.max(0, depth.current - 1);
        if (depth.current === 0) setOver(false);
      }}
      onDrop={(e) => {
        if (disabled || !hasFiles(e)) return;
        e.preventDefault();
        depth.current = 0;
        setOver(false);
        if (onDataTransfer) {
          onDataTransfer(e.dataTransfer);
          return;
        }
        const files = Array.from(e.dataTransfer.files);
        if (files.length > 0) onFiles?.(files);
      }}>
      {children}
      {over ? (
        <div
          data-testid="drop-overlay"
          className="pointer-events-none absolute inset-2 z-20 flex flex-col items-center justify-center gap-2 rounded-card border-2 border-dashed border-accent bg-accent-soft/90 p-4 text-center text-sm font-semibold text-accent-ink">
          {icon ? <span className="flex size-12 items-center justify-center rounded-full bg-accent-soft text-accent-ink">{icon}</span> : null}
          <span className="min-w-0 break-all">{label}</span>
        </div>
      ) : null}
    </div>
  );
}
