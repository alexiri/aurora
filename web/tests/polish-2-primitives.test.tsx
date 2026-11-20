import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { EmptyState, MasterDetail } from "@/components/states";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Badge } from "@/components/ui/badge";
import { LinkCard } from "@/components/ui/link-card";
import { LockPill } from "@/components/ui/lock-pill";
import { SettingField, SettingsCard, settingControlId } from "@/components/ui/setting-field";
import { StatusLine } from "@/components/ui/status-line";
import { Switch, SwitchField } from "@/components/ui/switch";
import { Unavailable } from "@/components/ui/unavailable";

vi.mock("next/link", () => ({
  default: ({ children, href, ...rest }: { children: React.ReactNode; href: string }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

// (P5-POLISH-2) Render tests per shared piece, in both themes (the markup is theme-independent: every colour is a token
// the theme swaps; axe runs over these pieces in the Playwright axe suites).
afterEach(() => document.documentElement.removeAttribute("data-theme"));

const who = { action: "change_this", role: "org_admin", people: [{ id: "u1", name: "Alice Admin" }], you_can: false, path: null } as never;

for (const theme of ["light", "dark"] as const) {
  describe(`${theme} theme`, () => {
    const themed = () => document.documentElement.setAttribute("data-theme", theme);

    it("LockPill is a neutral pill with the owner, and the who-can sentence for assistive technology", () => {
      themed();
      render(<LockPill owner="Set for you by an Aurora administrator" whoCan={who} />);
      const pill = screen.getByText(/Set for you by an Aurora administrator/);
      expect(pill.closest("span")?.className).toContain("bg-subtle");
      expect(pill.closest("span")?.className).not.toContain("pending");
      expect(screen.getByText(/Ask an organization admin/)).toBeInTheDocument();
    });

    it("LockPill with an href is a link", () => {
      themed();
      render(<LockPill owner="Set in Helm" href="/admin/settings" />);
      expect(screen.getByRole("link", { name: /Set in Helm/ })).toHaveAttribute("href", "/admin/settings");
    });

    it("AttentionChip renders its action next to the chip", () => {
      themed();
      render(<AttentionChip action={<button type="button">Review</button>}>Update available</AttentionChip>);
      expect(screen.getByText("Update available").className).toContain("bg-pending-soft");
      expect(screen.getByRole("button", { name: "Review" })).toBeInTheDocument();
    });

    it("Badge is sentence case in every tone", () => {
      themed();
      render(
        <>
          <Badge>Neutral</Badge>
          <Badge tone="accent">Accent</Badge>
          <Badge tone="danger">Danger</Badge>
        </>,
      );
      for (const t of ["Neutral", "Accent", "Danger"]) {
        expect(screen.getByText(t).className).not.toMatch(/uppercase|tracking-/);
      }
    });

    it("Switch is a labelled role=switch that toggles with the keyboard and the mouse", () => {
      themed();
      const seen: boolean[] = [];
      function Host() {
        const [on, setOn] = useState(false);
        return (
          <SwitchField
            id="x"
            label="Allow previews"
            hint="Turns the relay on."
            checked={on}
            onChange={(v) => {
              seen.push(v);
              setOn(v);
            }}
          />
        );
      }
      render(<Host />);
      const sw = screen.getByRole("switch", { name: "Allow previews" });
      expect(sw).not.toBeChecked();
      expect(sw).toHaveAccessibleDescription("Turns the relay on.");
      fireEvent.click(sw);
      expect(sw).toBeChecked();
      fireEvent.click(screen.getByText("Allow previews"));
      expect(seen).toEqual([true, false]);
    });

    it("Switch honours disabled", () => {
      themed();
      render(<Switch aria-label="Off" checked={false} onCheckedChange={() => {}} disabled />);
      expect(screen.getByRole("switch", { name: "Off" })).toBeDisabled();
    });

    it("StatusLine shows a dot, the sentence and an action", () => {
      themed();
      render(
        <StatusLine tone="attention" action={<button type="button">Reconnect</button>}>
          The sign-in expired
        </StatusLine>,
      );
      expect(screen.getByText("The sign-in expired")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Reconnect" })).toBeInTheDocument();
    });

    it("LinkCard is a link with its title and line", () => {
      themed();
      render(
        <LinkCard href="/o/acme" title="Add people">
          Invite your team.
        </LinkCard>,
      );
      const a = screen.getByRole("link", { name: /Add people/ });
      expect(a).toHaveAttribute("href", "/o/acme");
      expect(a).toHaveTextContent("Invite your team.");
    });

    it("Unavailable is one card with a title, the missing piece, who fixes it and the guide", () => {
      themed();
      render(
        <Unavailable title="The web terminal isn't available on this Aurora" fixer="An operator installs it." guide={<a href="/guide">Guide</a>}>
          The runner has no stream listener.
        </Unavailable>,
      );
      const card = screen.getByRole("region", { name: "The web terminal isn't available on this Aurora" });
      expect(card).toHaveTextContent("The runner has no stream listener.");
      expect(card).toHaveTextContent("An operator installs it.");
      expect(screen.getByRole("link", { name: "Guide" })).toBeInTheDocument();
      expect(screen.queryByRole("textbox")).toBeNull();
    });

    it("EmptyState has a title, a sentence and one action; for a viewer who cannot act, who can and Ask", () => {
      themed();
      const onAsk = vi.fn();
      const { rerender } = render(
        <EmptyState title="Nothing yet" action={<button type="button">New memory</button>}>
          It saves notes here.
        </EmptyState>,
      );
      expect(screen.getByRole("heading", { name: "Nothing yet" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "New memory" })).toBeInTheDocument();
      rerender(
        <EmptyState title="Nothing yet" action={<button type="button">New memory</button>} ask={{ whoCan: who, canAct: false, onAsk }}>
          It saves notes here.
        </EmptyState>,
      );
      expect(screen.queryByRole("button", { name: "New memory" })).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Ask" }));
      expect(onAsk).toHaveBeenCalled();
    });

    it("MasterDetail hides the detail pane while the list is empty", () => {
      themed();
      const { rerender } = render(<MasterDetail count={0} empty={<p>Empty</p>} list={<p>List</p>} detail={<p>Select a skill to edit it.</p>} />);
      expect(screen.getByText("Empty")).toBeInTheDocument();
      expect(screen.queryByText(/Select a skill/)).toBeNull();
      rerender(<MasterDetail count={2} empty={<p>Empty</p>} list={<p>List</p>} detail={<p>Select a skill to edit it.</p>} />);
      expect(screen.getByText(/Select a skill/)).toBeInTheDocument();
    });

    it("SettingField is a two-column row; a locked one has the Set in Helm pill and no control; SettingsCard carries one guide", () => {
      themed();
      render(
        <SettingsCard title="Limits" description="How much." guide="Limits">
          <SettingField id="a.b" label="Setting A" hint="Help text." source="dashboard" onReset={() => {}} defaultLabel="5">
            <input id={settingControlId("a.b")} />
          </SettingField>
          <SettingField id="c.d" label="Setting C" source="helm" helmKey="x.y" lockedValue="10" />
        </SettingsCard>,
      );
      expect(screen.getAllByText(/Guide: “Limits”/)).toHaveLength(1);
      expect(screen.getByLabelText("Setting A")).toBeInTheDocument();
      expect(screen.getByText("Set by the installation")).toBeInTheDocument();
      expect(screen.getByTestId("c.d-locked-value")).toHaveTextContent("10");
      expect(screen.queryByText(/in the operator guide/)).toBeNull();
    });
  });
}
