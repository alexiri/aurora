import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ApprovalNotifications } from "@/components/account/approval-notifications";
import type { Me } from "@/lib/api/queries";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

vi.mock("@/components/account/push-devices", () => ({ PushDevices: () => null }));

const me = (admin: boolean, on = true) =>
  ({
    user: { id: "u1", email: "a@x.io", is_instance_admin: admin },
    approval_email: true,
    admin_attention_email: on,
    default_org_id: null,
    memberships: [],
  }) as unknown as Me;

// FIX-167 Part B (C-1024): the switch is shown to instance admins only and patches /v1/me.
describe("Account › Notifications: administration e-mail switch", () => {
  const label = /E-mail me when something needs my decision in Administration/;

  it("is not shown to a person who is not an instance admin", () => {
    wrap(<ApprovalNotifications me={me(false)} />);
    expect(screen.queryByLabelText(label)).toBeNull();
  });

  it("is shown to an instance admin, reflects the stored value and patches it", async () => {
    let body: unknown = null;
    mockFetch({
      "PATCH /v1/me": async (req) => {
        body = await req.clone().json();
        return json(200, me(true, false));
      },
    });
    wrap(<ApprovalNotifications me={me(true)} />);
    const box = screen.getByLabelText(label) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    await waitFor(() => expect(body).toEqual({ admin_attention_email: false }));
  });
});
