import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TopicPage } from "@/components/topic-page";
import { EmbeddedPage, PageHeader } from "@/components/ui/page-header";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("PageHeader level (D-389)", () => {
  it("carries no level pill of its own: the rail's switcher says where you are", () => {
    render(<PageHeader title="People" description="Who is here." />);
    expect(screen.queryByTestId("level-badge")).toBeNull();
    expect(screen.getByRole("heading", { level: 1, name: "People" })).toBeInTheDocument();
  });

  it("a page the switcher does not place reads its level as plain text above the title, not a pill", () => {
    render(<PageHeader title="People" level={{ kind: "workspace", label: "Acme › Main" }} />);
    expect(screen.getByTestId("level-badge").tagName).toBe("P");
  });

  it("shows the level of the page next to its title", () => {
    render(<PageHeader title="People" level={{ kind: "workspace", label: "Acme › Main" }} />);
    const badge = screen.getByTestId("level-badge");
    expect(badge).toHaveTextContent("Acme › Main");
    expect(badge).toHaveAttribute("data-level", "workspace");
    expect(screen.getByRole("heading", { level: 1, name: "People" })).toBeInTheDocument();
  });

  it("labels Administration and an organization", () => {
    render(<PageHeader title="Data" level={{ kind: "instance", label: "Administration" }} />);
    expect(screen.getByTestId("level-badge")).toHaveAttribute("data-level", "instance");
    cleanup();
    render(<PageHeader title="Data" level={{ kind: "org", label: "Acme" }} />);
    expect(screen.getByTestId("level-badge")).toHaveTextContent("Acme");
  });

  it("an embedded header keeps its description and actions but adds no second title or badge", () => {
    render(
      <EmbeddedPage>
        <PageHeader title="Old page title" description="What it does." actions={<button>Act</button>} level={{ kind: "org", label: "Acme" }} />
      </EmbeddedPage>,
    );
    expect(screen.queryByRole("heading")).toBeNull();
    expect(screen.queryByTestId("level-badge")).toBeNull();
    expect(screen.getByText("What it does.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Act" })).toBeInTheDocument();
  });
});

describe("TopicPage", () => {
  const tabs = [
    { id: "one", label: "One", content: <p>first content</p> },
    { id: "two", label: "Two", content: <p>second content</p> },
  ] as const;

  it("opens the first tab, shows only the selected tab's content and writes ?tab=", () => {
    render(<TopicPage idBase="t" title="Topic" tabs={tabs} />);
    expect(screen.getByRole("heading", { level: 1, name: "Topic" })).toBeInTheDocument();
    expect(screen.getByText("first content")).toBeInTheDocument();
    expect(screen.queryByText("second content")).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Two" }));
    expect(screen.getByText("second content")).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=two");
  });

  it("opens the tab the address names, and falls back on an unknown one", () => {
    window.history.replaceState(null, "", "/?tab=two");
    render(<TopicPage idBase="t" title="Topic" tabs={tabs} />);
    expect(screen.getByText("second content")).toBeInTheDocument();
    cleanup();
    window.history.replaceState(null, "", "/?tab=nope");
    render(<TopicPage idBase="t" title="Topic" tabs={tabs} />);
    expect(screen.getByText("first content")).toBeInTheDocument();
  });

  it("follows ?tab= when a client navigation changes only the search parameter (org-2: Set up on Google Drive)", () => {
    window.history.replaceState(null, "", "/?tab=apps");
    const apps = [
      { id: "apps", label: "Apps", content: <p>apps content</p> },
      { id: "oauth-clients", label: "Sign-in apps", content: <p>sign-in apps content</p> },
    ] as const;
    const { rerender } = render(<TopicPage idBase="t" title="Topic" tabs={apps} />);
    expect(screen.getByText("apps content")).toBeInTheDocument();
    window.history.pushState(null, "", "/?tab=oauth-clients&client_key=google");
    rerender(<TopicPage idBase="t" title="Topic" tabs={apps} />);
    expect(screen.getByText("sign-in apps content")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Sign-in apps" })).toHaveAttribute("aria-selected", "true");
  });

  it("drops the dialog parameters of a tab when another tab opens", () => {
    window.history.replaceState(null, "", "/?tab=one&connect=github");
    render(<TopicPage idBase="t" title="Topic" tabs={tabs} />);
    fireEvent.click(screen.getByRole("tab", { name: "Two" }));
    expect(window.location.search).toBe("?tab=two");
  });

  it("a topic with one tab has no tab bar", () => {
    render(<TopicPage idBase="t" title="Topic" tabs={[tabs[0]]} />);
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.getByText("first content")).toBeInTheDocument();
  });
});
