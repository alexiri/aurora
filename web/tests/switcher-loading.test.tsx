import { cleanup, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ScopeSwitcher } from "@/components/nav/scope-switcher";
import { wrap } from "./support/render";

// P5-REFINE-16 firstday-2: while the session loads, the card is a disabled, busy "Loading workspaces…", not a live-looking button.

vi.mock("next/navigation", () => ({ useParams: () => ({}), usePathname: () => "/", useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("scope switcher while loading", () => {
  it("is busy and disabled, with no chevron button, until the session answers", async () => {
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(() => {})));
    wrap(<ScopeSwitcher place={{ kind: "workspace" }} />);
    const card = await screen.findByTestId("scope-switcher");
    expect(card).toHaveAttribute("aria-busy", "true");
    expect(card).toHaveAttribute("aria-disabled", "true");
    expect(card).toHaveTextContent("Loading workspaces…");
    expect(screen.queryByRole("button")).toBeNull();
  });
});
