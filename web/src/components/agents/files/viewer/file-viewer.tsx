"use client";

// The Files viewer (P5-FILES-3, D-316(f)(g), C-905, SEC-126): an editor for UTF-8 text (saved with an etag; a changed file
// opens the conflict dialog), a preview for the four raster image types, a "No preview" card for everything else. Nothing
// of a file is ever rendered as HTML, and no file is opened in a new tab: text goes into the editor, images are shown
// through a `data:` URL of a type decided by the first bytes.
import { useQuery } from "@tanstack/react-query";
import dynamic from "next/dynamic";
import { useEffect, useMemo, useRef, useState } from "react";

import { Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { ArrowLeftIcon, DownloadIcon } from "@/components/ui/icons";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import {
  IMAGE_PREVIEW_MAX,
  WAKE_ATTEMPTS,
  baseName,
  downloadUrl,
  fetchContent,
  filesErrorMessage,
  filesKeys,
  filesTiming,
  getFileText,
  humanSize,
  isTransient,
  putFile,
  type FileText,
} from "@/lib/api/files";

import { useUndoToast } from "@/components/ui/undo-toast";

import { setLeaveGuard } from "../leave-gate";
import { CsvTable } from "./csv";
import { imageDataUrl, sniffImage, typeGuess } from "./image";

// The editor (CodeMirror and its languages) is a separate chunk: the Files listing never pays for it.
const CodeEditor = dynamic(() => import("./editor"), { ssr: false, loading: () => <Loading label="Loading editor…" /> });

/** Esc, then Tab within this many ms leaves the editor (the terminal's WCAG 2.1.2 rule). */
export const ESCAPE_CHORD_MS = 1500;

export interface FileViewerProps {
  agent: string;
  path: string;
  /** The size from the listing, when known. */
  size?: number;
  /** The entry is not writable (`writable: false`, or under `.aurora`): the editor opens read-only. */
  readOnly?: boolean;
  /** A file that does not exist yet (New file): an empty editor whose first Save creates it. */
  isNew?: boolean;
  onBack: () => void;
  /** Whether there are unsaved changes; the page asks before leaving (and the browser before unloading). */
  onDirtyChange?: (dirty: boolean) => void;
}

export function FileViewer(props: FileViewerProps) {
  return props.isNew ? <EditorPane {...props} data={null} version={0} /> : <ExistingFile {...props} />;
}

function ExistingFile(props: FileViewerProps) {
  const { agent, path } = props;
  const [version, setVersion] = useState(0);
  // The text is read once per opening (and again on Reload); it is never kept in the cache or refetched behind the editor.
  const query = useQuery({
    queryKey: [...filesKeys.all(agent), "text", path],
    // (C-1060) `preview=1`: a text file over 1 MiB answers its first 256 KiB with `truncated`, never an etag (read-only).
    queryFn: () => getFileText(agent, path, { preview: true }),
    retry: (count, err) => isTransient(err) && count < WAKE_ATTEMPTS,
    retryDelay: filesTiming.retryDelayMs,
    staleTime: Infinity,
    gcTime: 0,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    refetchOnMount: "always",
  });
  const err = query.error;
  if (err instanceof ApiError && err.code === "file_not_editable") return <NonText {...props} reason={filesErrorMessage(err)} />;
  if (err) {
    return (
      <Notice tone="danger" role="alert" className="space-y-2">
        <p className="break-words">{filesErrorMessage(err)}</p>
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={props.onBack}>
            <ArrowLeftIcon /> Back to folder
          </Button>
        </div>
      </Notice>
    );
  }
  if (!query.data) {
    return query.isFetching && query.failureCount > 0 && isTransient(query.failureReason) ? (
      <Notice tone="pending" role="status">
        Waking the computer…
      </Notice>
    ) : (
      <Loading label="Opening the file…" />
    );
  }
  return (
    <EditorPane
      {...props}
      key={version}
      data={query.data}
      version={version}
      onReload={async () => {
        const next = await query.refetch({ throwOnError: true });
        if (next.data) setVersion((v) => v + 1);
      }}
    />
  );
}

const lineEndingOf = (data: FileText | null): "lf" | "crlf" => (data?.line_ending === "crlf" ? "crlf" : "lf");

function EditorPane({
  agent,
  path,
  readOnly = false,
  isNew = false,
  data,
  onBack,
  onDirtyChange,
  onReload,
}: FileViewerProps & { data: FileText | null; version: number; onReload?: () => Promise<void> }) {
  const initial = data?.content ?? "";
  const name = baseName(path);
  const toast = useUndoToast();
  // (computer-11) A preview of the start of a big file is read-only and says so; a CSV opens as a table first.
  const truncated = data?.truncated === true;
  const [asText, setAsText] = useState(false);
  const csv = !isNew && /\.csv$/i.test(name) && !asText;
  readOnly = readOnly || truncated;
  const [text, setText] = useState(initial);
  const [saved, setSaved] = useState(initial);
  const [etag, setEtag] = useState<string | null>(data?.etag ?? null);
  const [exists, setExists] = useState(!isNew);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [conflictNote, setConflictNote] = useState<string | null>(null);
  const [left, setLeft] = useState(false);
  const lastEscape = useRef(0);
  // The control after the editor that takes focus when Esc, Tab leaves it (the Back button).
  const exitFocus = useRef<HTMLDivElement>(null);
  // Save is reached from the editor's key binding, which was bound at mount: always call the latest closure.
  const latest = useRef<() => void>(() => {});

  const dirty = !readOnly && text !== saved;
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  async function save(how: "normal" | "overwrite" = "normal"): Promise<boolean> {
    if (saving || readOnly) return false;
    setSaving(true);
    setError(null);
    const body = text;
    try {
      const opts = how === "overwrite" || (exists && !etag) ? { replace: true } : exists ? { ifMatch: `"${etag}"` } : {};
      const res = await putFile(agent, path, body, opts);
      setEtag(res.etag ?? null);
      setExists(true);
      setSaved(body);
      setConflict(false);
      // (computer-10) "Saved" is a toast: a status line here moved the editor.
      toast(`Saved ${name}`);
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.code === "file_changed") {
        setConflictNote(null);
        setConflict(true);
      } else if (e instanceof ApiError && e.code === "file_exists") {
        setError("A file with that name already exists in this folder. Choose another name or open that file.");
      } else {
        setError(filesErrorMessage(e));
      }
      return false;
    } finally {
      setSaving(false);
    }
  }
  useEffect(() => {
    latest.current = () => {
      if (dirty) void save();
    };
  });
  // Leaving by any in-page way asks first (Back, another file, the breadcrumb, the page's own links): see `leave-gate`.
  const saveRef = useRef(save);
  const textRef = useRef(text);
  useEffect(() => {
    saveRef.current = save;
    textRef.current = text;
  });
  useEffect(() => {
    if (!dirty) return;
    setLeaveGuard({ name, save: () => saveRef.current(), discard: () => setSaved(textRef.current) });
    return () => setLeaveGuard(null);
  }, [dirty, name]);

  const copyMine = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setConflictNote("Your text is on the clipboard.");
    } catch {
      setConflictNote("Copying is not allowed in this browser. Select your text in the editor and copy it yourself.");
    }
  };

  return (
    // (P5-REFINE-19) The editor keeps its keys: no letter shortcut fires in it.
    <div className="space-y-3" data-no-shortcuts="">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h2 className="break-all font-display text-base font-semibold text-ink">{name}</h2>
          {readOnly ? <Badge>Read-only</Badge> : null}
          {/* (computer-4) A dot while there are unsaved changes; its words are the title and a hidden label. */}
          {dirty ? (
            <span className="inline-flex items-center text-pending-ink" title="Unsaved changes" data-testid="dirty-dot">
              <span aria-hidden="true">●</span>
              <span className="sr-only">Unsaved changes</span>
            </span>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {readOnly ? null : (
            <Button type="button" variant="primary" size="sm" onClick={() => void save()} disabled={saving || !dirty}>
              {saving ? "Saving…" : "Save"}
            </Button>
          )}
          {exists ? (
            <a href={downloadUrl(agent, path)} download onClick={() => toast(`Downloaded ${name}`)} className={buttonVariants({ variant: "outline", size: "sm" })}>
              <DownloadIcon /> Download
            </a>
          ) : null}
        </div>
      </div>
      <p className="break-all font-mono text-xs text-muted">{path}</p>

      {error ? (
        <Notice tone="danger" role="alert">
          <span className="break-words">{error}</span>
        </Notice>
      ) : null}
      {truncated ? (
        <Notice tone="neutral" role="status" className="flex flex-wrap items-center justify-between gap-2" data-testid="preview-note">
          <span>
            Showing the first {humanSize(new TextEncoder().encode(initial).length)} of {humanSize(data?.size ?? 0)}.
          </span>
          <a href={downloadUrl(agent, path)} download className={buttonVariants({ variant: "outline", size: "sm" })}>
            <DownloadIcon /> Download the whole file
          </a>
        </Notice>
      ) : null}

      {csv ? (
        <CsvTable name={name} text={initial} downloadHref={downloadUrl(agent, path)} onShowText={() => setAsText(true)} />
      ) : (
      <div
        className="h-[min(65dvh,40rem)] min-h-64 overflow-hidden rounded-card border border-line bg-surface"
        // Esc, then Tab leaves the editor (Tab on its own indents).
        onKeyDownCapture={(e) => {
          if (e.key === "Escape") {
            lastEscape.current = Date.now();
            setLeft(false);
          } else if (e.key === "Tab" && Date.now() - lastEscape.current < ESCAPE_CHORD_MS) {
            lastEscape.current = 0;
            e.preventDefault();
            e.stopPropagation();
            exitFocus.current?.querySelector("button")?.focus();
            setLeft(true);
          }
        }}>
        <CodeEditor
          initial={initial}
          lineEnding={lineEndingOf(data)}
          name={name}
          readOnly={readOnly}
          label={`Contents of ${name}`}
          onChange={setText}
          onSave={() => latest.current()}
        />
      </div>
      )}
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <div ref={exitFocus}>
          <Back onBack={onBack} />
        </div>
        {csv ? null : (
          <>
            <p className="text-xs text-muted">Tab indents. Press Esc, then Tab to leave the editor.</p>
            <p role="status" className="text-xs text-ink-2">
              {left ? "Left the editor." : ""}
            </p>
          </>
        )}
      </div>

      <Dialog open={conflict} onOpenChange={setConflict}>
        <DialogContent title="This file changed in the sandbox after you opened it." description="Someone or something wrote to it since. Choose what happens to your changes.">
          <div className="space-y-3">
            {conflictNote ? (
              <p role="status" className="text-sm text-ink-2">
                {conflictNote}
              </p>
            ) : null}
            <div className="flex flex-wrap justify-end gap-2">
              <Button type="button" variant="outline" onClick={() => void copyMine()}>
                Copy my text
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  setConflict(false);
                  void onReload?.().catch((e: unknown) => setError(filesErrorMessage(e)));
                }}>
                Reload
              </Button>
              <Button type="button" variant="danger" disabled={saving} onClick={() => void save("overwrite")}>
                Overwrite
              </Button>
            </div>
            <p className="text-xs text-muted">Reload discards your changes and shows the file as it is now. Overwrite replaces the file with your text.</p>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** A file the editor refused (binary, too large, not UTF-8): an image is previewed, anything else gets "No preview". */
function NonText({ agent, path, size, onBack, reason }: FileViewerProps & { reason: string }) {
  const tooBig = size !== undefined && size > IMAGE_PREVIEW_MAX;
  const content = useQuery({
    queryKey: [...filesKeys.all(agent), "content", path],
    queryFn: () => fetchContent(agent, path),
    enabled: !tooBig,
    retry: (count, e) => isTransient(e) && count < WAKE_ATTEMPTS,
    retryDelay: filesTiming.retryDelayMs,
    staleTime: Infinity,
    gcTime: 0,
    refetchOnWindowFocus: false,
  });
  const bytes = content.data ?? null;
  const mime = bytes ? sniffImage(bytes) : null;
  const [dims, setDims] = useState<string | null>(null);
  const url = useMemo(() => (bytes && mime ? imageDataUrl(bytes, mime) : null), [bytes, mime]);

  if (!tooBig && content.isPending) return <Loading label="Opening the file…" />;
  const shownSize = size ?? bytes?.length;
  if (url && mime) {
    return (
      <Card className="space-y-3 p-4">
        <Header agent={agent} path={path} />
        <p className="text-sm text-ink-2">
          {mime.replace("image/", "").toUpperCase()} image{dims ? `, ${dims}` : ""}
          {shownSize !== undefined ? `, ${humanSize(shownSize)}` : ""}
        </p>
        <div className="overflow-auto rounded-control border border-line bg-subtle p-2">
          {/* A data: URL of a type decided by the file's first bytes; `alt` is plain text. */}
          {/* eslint-disable-next-line @next/next/no-img-element -- a data: URL: next/image cannot optimise it */}
          <img
            src={url}
            alt={baseName(path)}
            className="mx-auto max-h-[70dvh] max-w-full object-contain"
            onLoad={(e) => setDims(`${e.currentTarget.naturalWidth} × ${e.currentTarget.naturalHeight} px`)}
          />
        </div>
        <Back onBack={onBack} />
      </Card>
    );
  }
  const loadFailed = content.error && !(content.error instanceof ApiError && content.error.code === "file_not_editable");
  return (
    <Card className="space-y-3 p-4">
      <Header agent={agent} path={path} />
      <h3 className="font-display text-base font-semibold text-ink">No preview for this file</h3>
      <p className="text-sm text-ink-2">
        {shownSize !== undefined ? `${humanSize(shownSize)} · ` : ""}
        {typeGuess(baseName(path))}
      </p>
      <p className="text-sm text-ink-2">{reason}</p>
      {tooBig ? <p className="text-sm text-ink-2">Images are previewed up to {humanSize(IMAGE_PREVIEW_MAX)}.</p> : null}
      {loadFailed ? <p className="text-sm text-danger-ink">{filesErrorMessage(content.error)}</p> : null}
      <Back onBack={onBack} />
    </Card>
  );
}

function Header({ agent, path }: { agent: string; path: string }) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-2">
      <div className="min-w-0">
        <h2 className="break-all font-display text-base font-semibold text-ink">{baseName(path)}</h2>
        <p className="break-all font-mono text-xs text-muted">{path}</p>
      </div>
      <a href={downloadUrl(agent, path)} download className={buttonVariants({ variant: "primary", size: "sm" })}>
        <DownloadIcon /> Download
      </a>
    </div>
  );
}

function Back({ onBack }: { onBack: () => void }) {
  return (
    <Button type="button" variant="outline" size="sm" onClick={onBack}>
      <ArrowLeftIcon /> Back to folder
    </Button>
  );
}
