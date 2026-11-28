import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TechnicalDetails } from "@/components/ui/technical-details";
import { AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { onboardingKeys, type MeOnboarding } from "@/lib/api/onboarding";
import { mockFetch } from "./support/api";

// (D-405, D-406) The "Technical details" fold is machinery: Set up and above. A member at Use never sees it; the
// per-person "Show more controls" switch opens it. `admin` folds stay for instance administrators only.

const state = (more: boolean): MeOnboarding => ({ welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: [], more_controls: more });

function setup({ instanceAdmin, more, admin }: { instanceAdmin: boolean; more: boolean; admin?: boolean }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  qc.setQueryData(["me"], { user: { id: "u1", display_name: "Bob", email: "bob@example.test", is_instance_admin: instanceAdmin } });
  qc.setQueryData(onboardingKeys.me, state(more));
  render(
    <QueryClientProvider client={qc}>
      <AltitudeScope>
        <TechnicalDetails admin={admin}>run: 42</TechnicalDetails>
        <MoreControlsLine />
      </AltitudeScope>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  window.localStorage.clear();
  mockFetch({});
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("<TechnicalDetails> and the altitude", () => {
  it("is not rendered for a member at Use, and counts as one hidden control", () => {
    setup({ instanceAdmin: false, more: false });
    expect(screen.queryByText("Technical details")).toBeNull();
    expect(screen.queryByText("run: 42")).toBeNull();
    expect(screen.getByText(/1 more control hidden/)).toBeInTheDocument();
  });

  it("is rendered at Set up (the floor of an instance administrator)", () => {
    setup({ instanceAdmin: true, more: false });
    expect(screen.getByText("Technical details")).toBeInTheDocument();
  });

  it("is rendered for a member who turned on Show more controls", () => {
    setup({ instanceAdmin: false, more: true });
    expect(screen.getByText("Technical details")).toBeInTheDocument();
  });

  it("an admin fold stays for instance administrators only, whatever the switch", () => {
    setup({ instanceAdmin: false, more: true, admin: true });
    expect(screen.queryByText("Technical details")).toBeNull();
    cleanup();
    setup({ instanceAdmin: true, more: false, admin: true });
    expect(screen.getByText("Technical details")).toBeInTheDocument();
  });
});
