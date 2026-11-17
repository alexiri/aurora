"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Section } from "@/components/ui/section";
import type { RuleInput } from "@/lib/api/policy";
import { commonChangeRule, hostOf, type CommonChange } from "@/lib/internet-access";

const CHANGES: { kind: CommonChange; title: string; text: string; confirm: string }[] = [
  {
    kind: "ask_before_changes",
    title: "Ask before changes on a website",
    text: "Agents ask you before sending anything that changes data on a site where they have no account.",
    confirm: "Agents ask you before they POST, PUT, PATCH or DELETE on",
  },
  {
    kind: "block",
    title: "Block a website",
    text: "No agent in this workspace can reach it.",
    confirm: "No agent can reach",
  },
];

/** The rule a Common change would add, in the words of a rule row. */
export function RulePreview({ rule }: { rule: RuleInput }) {
  const t = rule.targets?.[0];
  return (
    <p className="rounded-control border border-line bg-subtle p-3 text-sm text-ink" data-testid="common-change-rule">
      <span className="font-medium">{rule.name}</span>
      {t && t.type === "network" ? (
        <span className="mt-0.5 block font-mono text-xs text-muted">
          {t.methods?.length ? `${t.methods.join(" ")} · ` : ""}
          {t.host}
        </span>
      ) : null}
    </p>
  );
}

/**
 * (C-989) **Common changes**: two buttons that ask for a website, show the ordinary rule they would add, and add it to the
 * policy draft when confirmed. The rule appears in the rules list and the page's bar offers Apply.
 */
export function CommonChanges({ onAdd, scopeWords = "workspace" }: { onAdd: (rule: RuleInput) => void; scopeWords?: string }) {
  const [open, setOpen] = useState<CommonChange | null>(null);
  const [site, setSite] = useState("");
  const [shown, setShown] = useState<RuleInput | null>(null);
  const [error, setError] = useState<string | null>(null);
  const change = CHANGES.find((c) => c.kind === open);

  function close() {
    setOpen(null);
    setSite("");
    setShown(null);
    setError(null);
  }

  function preview() {
    const host = hostOf(site);
    if (!host || !open) return setError("Enter a website, like status.example.org.");
    setError(null);
    setShown(commonChangeRule(open, host));
  }

  return (
    <Section id="internet-common-changes" title="Common changes">
      <div className="grid gap-3 sm:grid-cols-2">
        {CHANGES.map((c) => (
          <button
            key={c.kind}
            type="button"
            onClick={() => setOpen(c.kind)}
            className="rounded-card border border-line bg-surface p-4 text-left text-ink shadow-card hover:bg-subtle"
          >
            <span className="block font-semibold">{c.title}</span>
            <span className="text-sm text-ink-2">{c.kind === "block" ? `No agent in this ${scopeWords} can reach it.` : c.text}</span>
          </button>
        ))}
      </div>
      {change ? (
        <Dialog open onOpenChange={(o) => !o && close()}>
          <DialogContent title={change.title} description="Enter the website. You will see the rule before it is added.">
            <form
              className="space-y-4"
              onSubmit={(e) => {
                e.preventDefault();
                if (shown) {
                  onAdd(shown);
                  close();
                } else preview();
              }}
            >
              <Field id="common-change-site" label="Website" error={error ?? undefined} hint="For example status.example.org">
                <input
                  id="common-change-site"
                  className={inputClass}
                  value={site}
                  autoFocus
                  onChange={(e) => {
                    setSite(e.target.value);
                    setShown(null);
                  }}
                />
              </Field>
              {shown ? (
                <div className="space-y-2">
                  <p className="text-sm text-ink-2">
                    {change.confirm} {hostOf(site)}. This rule will be added:
                  </p>
                  <RulePreview rule={shown} />
                </div>
              ) : null}
              <div className="flex flex-wrap justify-end gap-2">
                <Button type="button" variant="outline" onClick={close}>
                  Cancel
                </Button>
                <Button type="submit">{shown ? "Add rule" : "Show the rule"}</Button>
              </div>
            </form>
          </DialogContent>
        </Dialog>
      ) : null}
    </Section>
  );
}
