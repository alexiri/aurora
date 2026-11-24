import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentTemplateSection } from "@/components/templates/template-section";
import { json, mockFetch } from "./support/api";

// C-1117, C-1143: the agent's Template section on Settings › Manage — shown only for agents with a
// template or with helper files; the helper files with View (path, size, Executable, the text as
// plain text) and Remove (one or all, with a confirm); the "Made from" and update sentences.

const agent = {
  id: "a1",
  name: "Daneel",
  handle: "daneel",
  template: null,
} as { id: string; name: string; handle: string; template: { template_id: string; name: string; version: string } | null };

const withTemplate = { ...agent, template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" } };

const helpers = {
  items: [
    { path: "tidy.sh", size: 24, executable: true, sha256: "a".repeat(64) },
    { path: "notes.md", size: 120, executable: false, sha256: "b".repeat(64) },
  ],
};

function serve(extra: Record<string, (req: Request) => Response> = {}) {
  return mockFetch({
    "GET /v1/agents/a1/template": () => json(200, { template: agent === withTemplate ? { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" } : null, available: true, notes: [] }),
    "GET /v1/agents/a1/helpers": () => json(200, helpers),
    "GET /v1/agents/a1/helpers/content": (req) => {
      const path = new URL(req.url).searchParams.get("path") ?? "";
      return new Response(path === "tidy.sh" ? "#!/bin/sh\necho tidied\n" : "# notes\n", { status: 200 });
    },
    ...extra,
  });
}

function mount(a = agent) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AgentTemplateSection agent={a} agentBase="/o/o1/w/w1/agents/daneel" />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the agent's Template section (C-1117)", () => {
  it("shows itself for an agent with a template, with the Made from sentence", async () => {
    serve();
    mount(withTemplate);
    await waitFor(() => expect(screen.getByTestId("template-made-from")).toHaveTextContent("Made from Schedule manager 1.0.0"));
  });

  it("shows itself for an agent with helper files but no template", async () => {
    serve();
    mount();
    await waitFor(() => expect(screen.getByTestId("helper-count")).toHaveTextContent("2 helper files in .aurora/helpers"));
  });

  it("shows nothing for an agent with neither", async () => {
    serve({
      "GET /v1/agents/a1/helpers": () => json(200, { items: [] }),
    });
    mount();
    await waitFor(() => expect(screen.queryByTestId("agent-template-section")).not.toBeInTheDocument());
  });

  it("says the template is no longer offered when it is withdrawn", async () => {
    serve({
      "GET /v1/agents/a1/template": () => json(200, { template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" }, available: false, notes: [] }),
    });
    mount(withTemplate);
    await waitFor(() => expect(screen.getByTestId("template-unavailable")).toHaveTextContent("The template is no longer offered here. Daneel keeps working as it is."));
  });

  it("says a newer version is available and that the agent keeps working until it is applied", async () => {
    serve({
      "GET /v1/agents/a1/template": () =>
        json(200, {
          template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" },
          available: true,
          notes: [],
          update: { version: "1.1.0", content_sha256: "c".repeat(64), readme: "# New", changelog: null, inputs_needed: [], apps_added: [], parts: [] },
        }),
    });
    mount(withTemplate);
    await waitFor(() =>
      expect(screen.getByTestId("template-update")).toHaveTextContent("Version 1.1.0 is available. Daneel keeps working as it is until you apply it."),
    );
  });

  it("views the helper files: path, size, Executable, and the text as plain text", async () => {
    serve();
    mount();
    await screen.findByTestId("helper-count");
    fireEvent.click(screen.getByRole("button", { name: "View" }));
    const dlg = screen.getByRole("dialog", { name: "Helper files" });
    const tidy = within(dlg).getByTestId("helper-tidy.sh");
    expect(tidy.querySelector("summary")).toHaveTextContent("24 bytes · Executable");
    fireEvent.click(tidy.querySelector("summary")!);
    await waitFor(() => expect(within(tidy).getByLabelText("tidy.sh (executable) as text")).toHaveTextContent("#!/bin/sh"));
    // The other file is not executable and reads on demand.
    const notes = within(dlg).getByTestId("helper-notes.md");
    fireEvent.click(notes.querySelector("summary")!);
    await waitFor(() => expect(within(notes).getByLabelText("notes.md as text")).toHaveTextContent("# notes"));
  });

  it("removes one helper file after its confirm, then all of them", async () => {
    const deleted: string[] = [];
    serve({
      "DELETE /v1/agents/a1/helpers": (req) => {
        deleted.push(new URL(req.url).search);
        return json(204, null);
      },
    });
    mount();
    await screen.findByTestId("helper-count");
    fireEvent.click(screen.getByRole("button", { name: "View" }));
    const dlg = screen.getByRole("dialog", { name: "Helper files" });
    // One file: the confirm says what a template update does.
    fireEvent.click(within(dlg).getAllByRole("button", { name: "Remove" })[0]!);
    const one = screen.getByRole("dialog", { name: "Remove tidy.sh?" });
    expect(one).toHaveTextContent("If a template update offers it again, applying it brings it back.");
    fireEvent.click(within(one).getByRole("button", { name: "Remove file" }));
    await waitFor(() => expect(deleted).toEqual(["?path=tidy.sh"]));
    // All of them: the section's Remove with its own confirm.
    fireEvent.click(within(dlg).getAllByRole("button", { name: "Close" }).at(-1)!);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    const all = screen.getByRole("dialog", { name: "Remove all helper files?" });
    expect(all).toHaveTextContent("The 2 files in .aurora/helpers are removed from Daneel.");
    fireEvent.click(within(all).getByRole("button", { name: "Remove all" }));
    await waitFor(() => expect(deleted).toEqual(["?path=tidy.sh", ""]));
  });
});
