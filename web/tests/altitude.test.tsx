import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Altitude, AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { onboardingKeys, type MeOnboarding } from "@/lib/api/onboarding";
import { json, mockFetch, err } from "./support/api";

const state = (more: boolean): MeOnboarding => ({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: [], more_controls: more });

function setup(more: boolean | null, ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  if (more !== null) {
    qc.setQueryDefaults(onboardingKeys.me, { staleTime: Infinity });
    qc.setQueryData(onboardingKeys.me, state(more));
  }
  render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
  return qc;
}

beforeEach(() => {
  window.localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("<Altitude> (D-406)", () => {
  it("hides an Expert control at Use and shows it with the switch on", () => {
    setup(false, <Altitude id="agent.handle" level="expert">@bot</Altitude>);
    expect(screen.queryByText("@bot")).toBeNull();
    cleanup();
    setup(true, <Altitude id="agent.handle" level="expert">@bot</Altitude>);
    expect(screen.getByText("@bot")).toBeInTheDocument();
  });

  it("while the first answer is pending the switch reads off: no flash of Expert controls", () => {
    mockFetch({ "GET /v1/me/onboarding": () => new Promise<Response>(() => undefined) as never });
    setup(null, <Altitude id="agent.handle" level="expert">@bot</Altitude>);
    expect(screen.queryByText("@bot")).toBeNull();
  });

  it("with a label draws a collapsed line that opens just that control in place", () => {
    setup(
      false,
      <AltitudeScope>
        <Altitude id="agent.roles" level="expert" label="Model roles, effort and agent program">
          <p>Effort picker</p>
        </Altitude>
        <MoreControlsLine />
      </AltitudeScope>,
    );
    expect(screen.getByText("Model roles, effort and agent program")).toBeInTheDocument();
    expect(screen.queryByText("Effort picker")).toBeNull();
    expect(screen.getByText(/1 more control hidden/)).toBeInTheDocument();
    // The collapsed line's own Show comes first; the footer's turns the switch on.
    fireEvent.click(screen.getAllByRole("button", { name: "Show" })[0]!);
    expect(screen.getByText("Effort picker")).toBeInTheDocument();
    // Opened in place: nothing is hidden any more, so the footer is gone.
    expect(screen.queryByText(/more control/)).toBeNull();
  });

  it("promote shows the children at Use (a failing-key fixture)", () => {
    const failing = true;
    setup(false, <Altitude id="agent.roles" level="expert" promote={failing}>The key is missing</Altitude>);
    expect(screen.getByText("The key is missing")).toBeInTheDocument();
  });

  it("a control the role does not allow stays hidden with the switch on, and is not counted", () => {
    setup(
      true,
      <AltitudeScope>
        <Altitude id="agent.handle" level="expert" allowed={false}>owner only</Altitude>
        <MoreControlsLine />
      </AltitudeScope>,
    );
    expect(screen.queryByText("owner only")).toBeNull();
    expect(screen.getByText(/Showing more controls/)).toBeInTheDocument();
  });

  it("throws on an id the registry does not know (a build-time error in tests)", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    expect(() => setup(false, <Altitude id={"nope" as never} level="setup">x</Altitude>)).toThrow(/unknown id/);
    spy.mockRestore();
  });

  it("counts hidden controls in the footer, and Show turns the per-person switch on (optimistic)", async () => {
    const patches: unknown[] = [];
    mockFetch({
      "PATCH /v1/me/onboarding": async (req) => {
        patches.push(await req.json());
        return json(200, state(true));
      },
    });
    setup(
      false,
      <AltitudeScope>
        <Altitude id="agent.handle" level="expert"><p>one</p></Altitude>
        <Altitude id="agent.tests" level="expert"><p>two</p></Altitude>
        <Altitude id="agent.ssh" level="expert"><p>three</p></Altitude>
        <MoreControlsLine />
      </AltitudeScope>,
    );
    await screen.findByText(/3 more controls hidden/);
    fireEvent.click(screen.getByRole("button", { name: "Show" }));
    expect(await screen.findByText("one")).toBeInTheDocument();
    expect(screen.getByText(/Showing more controls/)).toBeInTheDocument();
    expect(patches).toEqual([{ more_controls: true }]);
    expect(screen.getByRole("button", { name: "Hide" })).toBeInTheDocument();
  });

  it("puts the switch back and shows the standard error when saving fails", async () => {
    mockFetch({ "PATCH /v1/me/onboarding": () => err(500, "internal") });
    setup(
      false,
      <AltitudeScope>
        <Altitude id="agent.handle" level="expert">one</Altitude>
        <MoreControlsLine />
      </AltitudeScope>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Show" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(screen.queryByText("one")).toBeNull();
    expect(screen.getByText(/1 more control hidden/)).toBeInTheDocument();
  });

  it("offers the switch once after three collapsed lines were opened in place", () => {
    const lines = ["agent.roles", "agent.tests", "agent.ssh"] as const;
    setup(
      false,
      <AltitudeScope>
        {lines.map((id) => (
          <Altitude key={id} id={id} level="expert" label={`line ${id}`}>
            <p>{id} body</p>
          </Altitude>
        ))}
        <MoreControlsLine />
      </AltitudeScope>,
    );
    expect(screen.queryByText(/Want everything/)).toBeNull();
    for (const b of screen.getAllByRole("button", { name: "Show" }).slice(0, 3)) fireEvent.click(b);
    expect(screen.getByText(/Want everything\? Turn on Show more controls\./)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/Want everything/)).toBeNull();
    expect(window.localStorage.getItem("aurora.altitude.hint-dismissed")).toBe("1");
  });
});
