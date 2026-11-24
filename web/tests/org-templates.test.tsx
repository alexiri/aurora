import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ORG_TEMPLATES_SENTENCE, OrgTemplates } from "@/components/templates/org-templates";
import { err, json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P6-TPL-4 Part A, C-1115: Organization › Agents › Templates — every offered template with its summary and a
// switch "Offered in <org>", the sentence on top, the empty list, and the instance kill switch in words.

const item = (over: Record<string, unknown> = {}) => ({
  template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work", enabled: true, version: "1.0.0",
  ...over,
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Organization templates tab (C-1115)", () => {
  it("says the sentence on top and lists each offered template with its summary and switch", async () => {
    mockFetch({
      "GET /v1/orgs/x/agent-templates": () =>
        json(200, { items: [item(), item({ template_id: "triage", name: "Triage", summary: "Sort the queue", enabled: false })] }),
    });
    wrap(<OrgTemplates orgId="x" orgName="Acme Labs" />);
    expect(await screen.findByTestId("org-templates")).toContainElement(screen.getByText(ORG_TEMPLATES_SENTENCE));
    const list = await screen.findByRole("list", { name: "Templates offered in Acme Labs" });
    const rows = within(list).getAllByTestId("org-template");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("Schedule manager")).toBeTruthy();
    expect(within(rows[0]).getByText("Triage and schedule the team's work")).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Offered in Acme Labs: Schedule manager" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Offered in Acme Labs: Triage" })).not.toBeChecked();
  });

  it("turns a template off: the switch sends enabled false and stays off", async () => {
    let items = [item()];
    let saved: unknown = null;
    mockFetch({
      "GET /v1/orgs/x/agent-templates": () => json(200, { items }),
      "PUT /v1/orgs/x/agent-templates/schedule-manager": async (req) => {
        saved = JSON.parse((await req.text()) || "null");
        items = [item({ enabled: false })];
        return json(200, items[0]);
      },
    });
    wrap(<OrgTemplates orgId="x" orgName="Acme Labs" />);
    const sw = await screen.findByRole("switch", { name: "Offered in Acme Labs: Schedule manager" });
    fireEvent.click(sw);
    await vi.waitFor(() => expect(saved).toEqual({ enabled: false }));
    await vi.waitFor(() => expect(sw).not.toBeChecked());
  });

  it("a failed switch says what went wrong and the switch goes back", async () => {
    mockFetch({
      "GET /v1/orgs/x/agent-templates": () => json(200, { items: [item()] }),
      "PUT /v1/orgs/x/agent-templates/schedule-manager": () => err(404, "templates_disabled", "no"),
    });
    wrap(<OrgTemplates orgId="x" orgName="Acme Labs" />);
    const sw = await screen.findByRole("switch", { name: "Offered in Acme Labs: Schedule manager" });
    fireEvent.click(sw);
    await vi.waitFor(() => expect(sw).toBeChecked());
    expect(await screen.findByText("Agent templates are turned off on this instance.")).toBeTruthy();
  });

  it("no offered templates reads as an empty state, not an error", async () => {
    mockFetch({ "GET /v1/orgs/x/agent-templates": () => json(200, { items: [] }) });
    wrap(<OrgTemplates orgId="x" orgName="Acme Labs" />);
    expect(await screen.findByText("Nothing offered yet")).toBeTruthy();
    expect(screen.getByText("Templates appear here when an instance administrator offers one.")).toBeTruthy();
  });

  it("the instance kill switch (404 templates_disabled) is words, never a red error", async () => {
    mockFetch({ "GET /v1/orgs/x/agent-templates": () => err(404, "templates_disabled", "templates are disabled") });
    wrap(<OrgTemplates orgId="x" orgName="Acme Labs" />);
    expect(await screen.findByText("Agent templates are turned off")).toBeTruthy();
    expect(screen.getByText("Agent templates are turned off on this instance. An Aurora administrator can turn them on again.")).toBeTruthy();
    expect(screen.queryByRole("switch")).toBeNull();
  });
});
