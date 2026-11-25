import { screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { EmergencyBanners } from "@/components/emergency/emergency-banners";
import { setDisplayTimeZone } from "@/lib/format-date";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

// (FIX-236, verify LOW-2) The reader's banner counts the caller's OWN grant down
// (`emergency_reader_expires_at`), never the newest grant in `audience.emergency`, which may
// belong to another owner; "End now" ends that same own grant (`emergency_reader_id`).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

beforeEach(() => setDisplayTimeZone("UTC"));

const otherOwnersGrant = {
  id: "e-other",
  by: { id: "u2", display_name: "Oscar Owner" },
  reason: "Another owner's hold",
  opened_at: "2026-11-20T14:30:00Z",
  expires_at: "2026-11-20T18:00:00Z",
  ended_at: null,
  active: true,
};

const now = "2026-11-20T10:00:00Z";

function serve(workspace: Record<string, unknown>) {
  mockFetch({
    "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "main", name: "Main" } }),
    "GET /v1/workspaces/w1": () =>
      json(200, {
        id: "w1", org_id: "o1", slug: "main", name: "Main", my_role: null, administers: true,
        memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of: null, frozen: null,
        ...workspace,
      }),
  });
}

describe("the emergency reader's banner (verify LOW-2)", () => {
  it("counts the caller's own grant down, not the newest grant's", async () => {
    serve({
      id: "w1",
      name: "Main",
      emergency_reader: true,
      emergency_reader_id: "e-own",
      // The caller's own grant ends at 16:00; the audience's newest (another owner's) at 18:00.
      emergency_reader_expires_at: "2026-11-20T16:00:00Z",
      audience: {
        people: 2,
        owners: [{ id: "u2", display_name: "Oscar Owner" }],
        owners_total: 1,
        emergency_active_more: 1,
        emergency: otherOwnersGrant,
      },
    });
    wrap(<EmergencyBanners />);
    const banner = await screen.findByTestId("emergency-reader-banner");
    expect(banner).toHaveTextContent("Emergency read access until 16:00.");
    expect(banner).not.toHaveTextContent("18:00");
  });

  it("keeps working when the own expiry is absent (falls back to the shown grant)", async () => {
    serve({
      id: "w1",
      name: "Main",
      emergency_reader: true,
      emergency_reader_id: "e-own",
      audience: {
        people: 2,
        owners: [{ id: "u2", display_name: "Oscar Owner" }],
        owners_total: 1,
        emergency_active_more: 0,
        emergency: otherOwnersGrant,
      },
    });
    wrap(<EmergencyBanners />);
    const banner = await screen.findByTestId("emergency-reader-banner");
    expect(banner).toHaveTextContent("Emergency read access until 18:00.");
  });
});
