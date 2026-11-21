"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { copyLabel, useCopy } from "@/lib/use-copy";
import { useOrigin } from "@/lib/use-origin";
import { authKind, callSentence, SNIPPET_LANGUAGES, webhookSnippet, type SnippetLanguage, type WebhookAuth } from "@/lib/webhook-snippets";

export { authKind };

/**
 * (P5-REFINE-14, webhooks-1) **How to call it**: what the sender must send, and a real snippet to copy (curl, Python, Node)
 * with the address and the header name. The secret is never in a snippet. On the card the full address was shown once, so
 * the snippet reads `$WEBHOOK_URL` and a comment names `<origin>/v1/hooks/<prefix>…`; with the real `url` (right after
 * creating or rotating) it uses that.
 */
export function HowToCall({
  url,
  prefix,
  kind,
  header,
  addressShownOnce = false,
  idBase = "howto",
}: {
  url?: string | null;
  prefix?: string | null;
  kind: WebhookAuth;
  header?: string | null;
  addressShownOnce?: boolean;
  idBase?: string;
}) {
  const [lang, setLang] = useState<SnippetLanguage>("curl");
  const { state, copy } = useCopy();
  const origin = useOrigin();
  const snippet = webhookSnippet(lang, { url, prefix, origin, kind, header });
  return (
    <section aria-label="How to call it" className="space-y-3 rounded-card border border-line bg-surface p-4">
      <div className="flex items-center justify-between gap-3">
        <h3 className="font-display text-base font-semibold text-ink">How to call it</h3>
        <Button type="button" size="sm" variant="outline" onClick={() => copy(snippet)} aria-label={`Copy the ${lang} example`}>
          {copyLabel(state)}
        </Button>
      </div>
      <p className="text-sm text-ink-2">{callSentence(kind, header)}</p>
      <Tabs
        items={SNIPPET_LANGUAGES.map((l) => ({ value: l.value, label: l.label }))}
        value={lang}
        onValueChange={(v) => setLang(v as SnippetLanguage)}
        label="Example language"
        idBase={idBase}
      />
      <pre
        role="tabpanel"
        id={tabPanelId(idBase, lang)}
        aria-labelledby={tabId(idBase, lang)}
        tabIndex={0}
        data-testid="howto-snippet"
        className="overflow-x-auto whitespace-pre rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink"
      >
        {snippet}
      </pre>
      {kind !== "none" ? <p className="text-xs text-muted">The secret is not in the example: the sender keeps it in {lang === "curl" ? "$SECRET" : "the WEBHOOK_SECRET environment variable"}.</p> : null}
      {addressShownOnce ? <p className="text-xs text-muted">The full address was shown once. Rotate it to get a new one.</p> : null}
    </section>
  );
}
