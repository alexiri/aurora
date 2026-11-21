import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { FreezeControl } from "@/components/freeze/freeze-panel";
import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { authorityRank, type Frozen } from "@/lib/api/freeze";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

const frozen = (over: Partial<Frozen> = {}): Frozen => ({
  at: "2026-01-02T03:04:05Z",
  by_label: "boss@example.com",
  by: { id: "u1", display_name: "Alice Admin" },
  reason: "compromised token",
  authority: "workspace_owner",
  level: "agent",
  ...over,
});

const noop = { mutateAsync: async () => undefined, isPending: false, error: null };

describe("authorityRank", () => {
  it("orders workspace_owner < org_admin < instance_admin", () => {
    expect(authorityRank("workspace_owner")).toBeLessThan(authorityRank("org_admin"));
    expect(authorityRank("org_admin")).toBeLessThan(authorityRank("instance_admin"));
  });
});

describe("FreezeControl", () => {
  it("requires a reason, lists the effects and sends the pause", async () => {
    const freeze = vi.fn(async () => undefined);
    wrap(
      <FreezeControl
        subject="agent"
        name="Bot"
        frozen={null}
        callerRank={1}
        freeze={{ mutateAsync: freeze, isPending: false, error: null }}
        unfreeze={{ mutateAsync: async () => undefined, isPending: false, error: null }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Pause agent" }));
    expect(await screen.findByText(/Running and queued turns are stopped/)).toBeInTheDocument();
    expect(screen.getByText(/they resume when you resume it/)).toBeInTheDocument();
    const confirm = screen.getByRole("button", { name: "Pause agent", hidden: false, description: "" });
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "  stolen token  " } });
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    await waitFor(() => expect(freeze).toHaveBeenCalledWith("stolen token"));
  });

  it("shows the banner and an enabled Resume when the caller's authority suffices", () => {
    wrap(<FreezeControl subject="agent" name="Bot" frozen={frozen()} callerRank={1} freeze={noop} unfreeze={noop} />);
    expect(screen.getByRole("status")).toHaveTextContent("Paused by Alice Admin on");
    expect(screen.getByRole("status")).not.toHaveTextContent("boss@example.com");
    expect(screen.getByRole("status")).toHaveTextContent("compromised token");
    expect(screen.getByRole("button", { name: "Resume agent" })).toBeEnabled();
  });

  it("a pause by a person stored by address shows a name; a pause without a person says only when (C-1102)", () => {
    const { unmount } = wrap(
      <FreezeControl subject="agent" name="Bot" frozen={frozen({ by: { id: "u2", display_name: "erin@example.test" } })} callerRank={1} freeze={noop} unfreeze={noop} />,
    );
    expect(screen.getByRole("status")).toHaveTextContent("Paused by Erin on");
    expect(screen.getByRole("status")).not.toHaveTextContent("@");
    unmount();
    wrap(<FreezeControl subject="agent" name="Bot" frozen={frozen({ by: null })} callerRank={1} freeze={noop} unfreeze={noop} />);
    expect(screen.getByRole("status")).toHaveTextContent(/^Paused on \S/);
    expect(screen.getByRole("status")).not.toHaveTextContent("boss@example.com");
  });

  it("disables Resume with the explanation when a higher authority paused it", () => {
    wrap(
      <FreezeControl
        subject="agent"
        name="Bot"
        frozen={frozen({ authority: "org_admin" })}
        callerRank={authorityRank("workspace_owner")}
        freeze={noop}
        unfreeze={noop}
      />,
    );
    const b = screen.getByRole("button", { name: "Resume agent" });
    expect(b).toBeDisabled();
    expect(screen.getByText(/A higher authority paused this/)).toBeInTheDocument();
  });

  it("offers no Resume for a pause inherited from the organization", () => {
    wrap(<FreezeControl subject="agent" name="Bot" frozen={frozen({ level: "org" })} callerRank={3} freeze={noop} unfreeze={noop} />);
    expect(screen.queryByRole("button", { name: /Resume/ })).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("because its organization is paused");
  });
});

describe("AgentFreeze", () => {
  it("posts the resume for the agent", async () => {
    const fn = mockFetch({
      "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: "owner", settings: {} }),
      "POST /v1/agents/a1/unfreeze": () => json(200, { frozen: null, effective_frozen: false }),
    });
    wrap(<AgentFreeze agentId="a1" name="Bot" orgId="o1" frozen={frozen()} canOwn />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Resume agent" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Resume agent" }));
    await waitFor(() =>
      expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && r.url.endsWith("/v1/agents/a1/unfreeze"))).toBe(true),
    );
  });

  it("members without can_own see the banner but no controls", () => {
    wrap(<AgentFreeze agentId="a1" name="Bot" orgId="o1" frozen={frozen()} canOwn={false} />);
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
