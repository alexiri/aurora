"use client";

import { Button } from "@/components/ui/button";
import { useCopy } from "@/lib/use-copy";

import { truncateHash } from "./fingerprint-diff";

const LOOKS_LIKE_HASH = /^[0-9a-f-]{16,}$/i;

/**
 * A hash or an id shown cut to its first characters, with a Copy action that carries the whole value. A value that is
 * not a hash (a word such as "not set") shows as it is.
 */
export function HashValue({ value, label }: { value: string; label: string }) {
  const { state, copy } = useCopy();
  if (!LOOKS_LIKE_HASH.test(value)) return <span>{value}</span>;
  return (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-1">
      <code className="font-mono text-xs" title={value}>
        {truncateHash(value)}
      </code>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        aria-label={`Copy ${label}`}
        onClick={() => copy(value)}
      >
        {state === "copied" ? "Copied" : state === "failed" ? "Select it instead" : "Copy"}
      </Button>
    </span>
  );
}
