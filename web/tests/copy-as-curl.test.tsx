import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { KeyTable } from "@/components/account/key-table";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { apiKeyCurl, shellQuote } from "@/lib/copy-as-curl";
import { webhookSnippet } from "@/lib/webhook-snippets";
import { wrap } from "./support/render";

// P5-REFINE-19 item 5: Copy as curl is an Expert control and its commands carry placeholders, never a secret or a key.

let copied: string[] = [];
beforeEach(() => {
  copied = [];
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(async (t: string) => void copied.push(t)) } });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the commands", () => {
  it("quotes for a shell", () => {
    expect(shellQuote("a'b")).toBe(`'a'\\''b'`);
  });

  it("calls /v1/me with the key's environment variable on the page's origin", () => {
    expect(apiKeyCurl("https://aurora.example.org")).toBe('curl -H "Authorization: Bearer $AURORA_API_KEY" https://aurora.example.org/v1/me');
    expect(apiKeyCurl("https://aurora.example.org/")).toContain("org/v1/me");
  });

  it("a webhook's curl is the generator's (signed: the signing recipe; bearer: the header; none: a plain POST)", () => {
    const signed = webhookSnippet("curl", { prefix: "auw_ab", origin: "https://a.example", kind: "hmac_sha256", header: "X-Signature" });
    expect(signed).toContain('openssl dgst -sha256 -hmac "$SECRET"');
    expect(signed).toContain('-H "X-Signature: sha256=$SIG"');
    expect(webhookSnippet("curl", { prefix: "auw_ab", kind: "bearer", header: "X-Token" })).toContain('-H "X-Token: Bearer $SECRET"');
    const none = webhookSnippet("curl", { prefix: "auw_ab", kind: "none" });
    expect(none).not.toContain("$SECRET");
    expect(none).not.toContain('-H "X-');
  });
});

const key = {
  id: "k1", name: "ci-script", kind: "service", org_id: "o", workspace_id: "w", display_prefix: "auk_ab",
  owner: { id: "u", display_name: "U" }, created_at: "2026-01-01T00:00:00Z", last_used_at: null, expires_at: null, revoked_at: null,
};

describe("API keys", () => {
  it("Copy as curl is in each active key's menu and copies a command with no key value", async () => {
    wrap(<><KeyTable keys={[key, { ...key, id: "k2", name: "old", revoked_at: "2026-02-01T00:00:00Z" }]} onRevoke={async () => undefined} /><UndoToastHost /></>);
    expect(screen.queryByRole("button", { name: "More for old" })).toBeNull();
    const trigger = screen.getByRole("button", { name: "More for ci-script" });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Copy as curl" }));
    await waitFor(() => expect(copied).toHaveLength(1));
    expect(copied[0]).toBe(`curl -H "Authorization: Bearer $AURORA_API_KEY" ${window.location.origin}/v1/me`);
    expect(copied[0]).not.toContain("auk_");
    expect(await screen.findByText("Copied the curl command")).toBeInTheDocument();
  });

  it("has no Copy as curl with Show more controls off", () => {
    wrap(<KeyTable keys={[key]} onRevoke={async () => undefined} />, { moreControls: false });
    expect(screen.queryByRole("button", { name: "More for ci-script" })).toBeNull();
    expect(screen.getByRole("button", { name: "Revoke ci-script" })).toBeInTheDocument();
  });
});
