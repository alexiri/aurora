import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { OpenEmergencyAccess } from "@/components/emergency/emergency-open";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

// (FIX-236, D-478, C-1164) The open dialog on the manage page: the reason's validation before
// anything is sent, the duration select (4 hours the default), then the danger step with the
// consequence in plain words, "Back" keeping the reason, and the POST on confirm.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "lab" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));

function serve(over: Record<string, (req: Request) => Response> = {}) {
  const calls: { method: string; path: string; body: unknown }[] = [];
  const fetchMock = mockFetch({
    "POST /v1/workspaces/w1/emergency-access": (req) => {
      // The handler itself records the body.
      return req
        .clone()
        .json()
        .then((body: unknown) => {
          calls.push({ method: "POST", path: "/v1/workspaces/w1/emergency-access", body });
          return over["POST /v1/workspaces/w1/emergency-access"]?.(req) ?? json(201, { id: "e1" });
        });
    },
    ...over,
  });
  return { calls, fetchMock };
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

function open() {
  return wrap(<OpenEmergencyAccess wsId="w1" wsName="Lab" onOpened={() => undefined} />);
}

const typeReason = (text: string) => fireEvent.change(screen.getByLabelText(/Reason/), { target: { value: text } });

describe("the open emergency read access dialog (FIX-236)", () => {
  it("the trigger opens the form: a reason and a duration, 4 hours by default", () => {
    serve();
    open();
    fireEvent.click(screen.getByTestId("open-emergency"));
    expect(screen.getByRole("dialog", { name: "Open emergency read access" })).toBeInTheDocument();
    expect(screen.getByLabelText(/Reason/)).toBeInTheDocument();
    const hours = screen.getByLabelText(/How long/) as HTMLSelectElement;
    expect(hours.value).toBe("4");
    expect(withinHours(hours).map((o) => o.value)).toEqual(["1", "2", "4", "8", "24"]);
    expect(withinHours(hours).find((o) => o.value === "4")?.label).toBe("4 hours (default)");
  });

  it("the reason is validated here: empty and too short are refused, nothing is sent", () => {
    serve();
    open();
    fireEvent.click(screen.getByTestId("open-emergency"));
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText("Say why you need to read this workspace.")).toBeInTheDocument();
    typeReason("too short");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText("A reason takes at least 10 characters.")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Open emergency read access" })).toBeInTheDocument();
  });

  it("a valid reason goes to the danger step, which names who is told and what is recorded", async () => {
    const { calls } = serve();
    open();
    fireEvent.click(screen.getByTestId("open-emergency"));
    typeReason("Legal hold on the account's records");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    const danger = await screen.findByRole("dialog", { name: "Open emergency read access to Lab?" });
    expect(danger).toHaveTextContent(
      "Everyone in Lab and the organization's other owners will be told, with your reason. Everything you open is recorded. You can read, not act.",
    );
    // Nothing has been sent yet.
    expect(calls).toHaveLength(0);
  });

  it("Back returns to the form with the reason kept; confirm POSTs the reason and the hours", async () => {
    const { calls } = serve();
    open();
    fireEvent.click(screen.getByTestId("open-emergency"));
    typeReason("Legal hold on the account's records");
    fireEvent.change(screen.getByLabelText(/How long/), { target: { value: "8" } });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    // "Back" keeps the reason; the form step is shown again.
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect((screen.getByLabelText(/Reason/) as HTMLTextAreaElement).value).toBe("Legal hold on the account's records");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    fireEvent.click(screen.getByRole("button", { name: "Open emergency read access to Lab" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]!.body).toEqual({ reason: "Legal hold on the account's records", hours: 8 });
  });

  it("a refusal (an active grant, 409) shows the server's words in the danger step", async () => {
    serve({
      "POST /v1/workspaces/w1/emergency-access": () => json(409, { error: { code: "emergency_access_active", message: "Emergency read access is already open here." } }),
    });
    open();
    fireEvent.click(screen.getByTestId("open-emergency"));
    typeReason("Legal hold on the account's records");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    fireEvent.click(await screen.findByRole("button", { name: "Open emergency read access to Lab" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Emergency read access is already open here.");
  });
});

function withinHours(select: HTMLSelectElement) {
  return Array.from(select.options).map((o) => ({ value: o.value, label: o.label }));
}
