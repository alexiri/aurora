"use client";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { useUndoToast } from "@/components/ui/undo-toast";
import { useCopy } from "@/lib/use-copy";

/**
 * (P5-REFINE-19, `copy.curl`, Expert) **Copy as curl** as a button (a webhook card). `command` is built with placeholders
 * (`lib/copy-as-curl.ts`): it never holds a secret.
 */
export function CopyCurlButton({ command, label = "Copy as curl" }: { command: string; label?: string }) {
  const { state, copy } = useCopy();
  return (
    <Altitude id="copy.curl" level="expert">
      <Button type="button" size="sm" variant="outline" onClick={() => copy(command)}>
        {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : label}
      </Button>
    </Altitude>
  );
}

/** **Copy as curl** in a row's menu (an API key's row): a "…" button that opens a one-entry menu. */
export function CopyCurlMenu({ command, subject }: { command: string | (() => string); subject: string }) {
  const toast = useUndoToast();
  return (
    <Altitude id="copy.curl" level="expert">
      <Menu>
        <MenuTrigger asChild>
          <Button type="button" size="sm" variant="outline" aria-label={`More for ${subject}`}>
            <MoreIcon className="size-4" aria-hidden="true" />
          </Button>
        </MenuTrigger>
        <MenuContent align="end">
          <MenuItem
            onSelect={() => {
              const write = navigator.clipboard?.writeText(typeof command === "function" ? command() : command);
              if (!write) return toast("Your browser blocked copying");
              write.then(
                () => toast("Copied the curl command"),
                () => toast("Your browser blocked copying"),
              );
            }}
          >
            Copy as curl
          </MenuItem>
        </MenuContent>
      </Menu>
    </Altitude>
  );
}
