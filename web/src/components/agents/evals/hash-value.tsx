"use client";

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";

import { truncateHash } from "./fingerprint-diff";

const LOOKS_LIKE_HASH = /^[0-9a-f-]{16,}$/i;

/**
 * A hash or an id shown cut to its first characters, with a Copy action that carries the whole value. A value that is
 * not a hash (a word such as "not set") shows as it is.
 */
export function HashValue({ value, label }: { value: string; label: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const t = setTimeout(() => setState("idle"), 2000);
    return () => clearTimeout(t);
  }, [state]);
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
        onClick={() => {
          const write = navigator.clipboard?.writeText(value);
          if (!write) return setState("failed");
          write.then(
            () => setState("copied"),
            () => setState("failed"),
          );
        }}
      >
        {state === "copied" ? "Copied" : state === "failed" ? "Select it instead" : "Copy"}
      </Button>
    </span>
  );
}
