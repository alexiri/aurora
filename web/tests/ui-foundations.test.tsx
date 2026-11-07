import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { WorkspaceNav } from "@/components/app-shell";
import { THEME_SCRIPT } from "@/components/theme/theme";
import { ThemeSwitch, ThemeSync } from "@/components/theme/theme-switch";
import { AuroraLogo, AuroraMark } from "@/components/ui/aurora-logo";
import { Badge } from "@/components/ui/badge";
import { Field } from "@/components/ui/field";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs } from "@/components/ui/tabs";

vi.mock("next/link", () => ({
  default: ({ children, href, ...rest }: { children: React.ReactNode; href: string }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

afterEach(() => {
  cleanup();
  localStorage.clear();
  document.cookie = "aurora_theme=; Max-Age=0; Path=/";
  document.documentElement.removeAttribute("data-theme");
});

describe("theme", () => {
  let dark = false;
  const listeners = new Set<() => void>();
  beforeEach(() => {
    dark = false;
    listeners.clear();
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        get matches() {
          return dark;
        },
        addEventListener: (_: string, cb: () => void) => listeners.add(cb),
        removeEventListener: (_: string, cb: () => void) => listeners.delete(cb),
      })),
    );
  });

  it("is a radio group with accessible names, defaulting to Match system", () => {
    render(<ThemeSwitch />);
    expect(screen.getByRole("radiogroup", { name: "Theme" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Match system" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Light" })).not.toBeChecked();
    expect(screen.getByRole("radio", { name: "Dark" })).not.toBeChecked();
  });

  it("persists the choice to localStorage and the cookie and sets data-theme", () => {
    render(<ThemeSwitch />);
    fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    expect(localStorage.getItem("aurora.theme")).toBe("dark");
    expect(document.cookie).toContain("aurora_theme=dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(screen.getByRole("radio", { name: "Dark" })).toBeChecked();
    fireEvent.click(screen.getByRole("radio", { name: "Light" }));
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("follows the OS live while on System", () => {
    render(
      <>
        <ThemeSync />
        <ThemeSwitch />
      </>,
    );
    expect(document.documentElement.dataset.theme).toBe("light");
    dark = true;
    listeners.forEach((cb) => cb());
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("does not follow the OS once a fixed theme is chosen", () => {
    render(
      <>
        <ThemeSync />
        <ThemeSwitch />
      </>,
    );
    fireEvent.click(screen.getByRole("radio", { name: "Light" }));
    expect(listeners.size).toBe(0);
  });

  it("head script is constant and resolves system before paint", () => {
    expect(THEME_SCRIPT).not.toContain("${");
    dark = true;
    new Function(THEME_SCRIPT)();
    expect(document.documentElement.dataset.theme).toBe("dark");
    localStorage.setItem("aurora.theme", "light");
    new Function(THEME_SCRIPT)();
    expect(document.documentElement.dataset.theme).toBe("light");
  });
});

describe("brand", () => {
  it("gives two logo instances distinct gradient ids", () => {
    const { container } = render(
      <>
        <AuroraMark />
        <AuroraLogo />
      </>,
    );
    const ids = [...container.querySelectorAll("linearGradient")].map((g) => g.id);
    expect(ids).toHaveLength(4);
    expect(new Set(ids).size).toBe(4);
  });

  it("names the logo Aurora", () => {
    render(<AuroraLogo />);
    expect(screen.getByRole("img", { name: "Aurora" })).toBeInTheDocument();
  });
});

describe("primitives", () => {
  it("Tabs expose tablist/tab semantics and arrow-key navigation", () => {
    function Harness() {
      const [v, setV] = useState("a");
      return (
        <Tabs
          label="Views"
          idBase="t"
          value={v}
          onValueChange={setV}
          items={[
            { value: "a", label: "Alpha" },
            { value: "b", label: "Beta" },
          ]}
        />
      );
    }
    render(<Harness />);
    expect(screen.getByRole("tablist", { name: "Views" })).toBeInTheDocument();
    const alpha = screen.getByRole("tab", { name: "Alpha" });
    expect(alpha).toHaveAttribute("aria-selected", "true");
    expect(alpha).toHaveAttribute("aria-controls", "t-panel-a");
    fireEvent.keyDown(alpha, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Beta" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Alpha" })).toHaveAttribute("aria-selected", "false");
  });

  it("Field links hint and error with aria-describedby and aria-invalid", () => {
    render(
      <Field id="x" label="Name" error="Required">
        <input id="x" />
      </Field>,
    );
    const input = screen.getByLabelText("Name");
    expect(input).toHaveAttribute("aria-describedby", "x-error");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("Required");
  });

  it("Badge and StatusDot always carry text", () => {
    render(
      <>
        <Badge tone="pending">Held</Badge>
        <StatusDot tone="accent">Running</StatusDot>
      </>,
    );
    expect(screen.getByText("Held")).toBeInTheDocument();
    expect(screen.getByText("Running")).toBeInTheDocument();
  });

  it("PageHeader renders breadcrumb, title, description and actions", () => {
    render(
      <PageHeader
        title="Agents"
        description="All agents"
        breadcrumb={[{ label: "Acme", href: "/o/acme" }, { label: "Agents" }]}
        actions={<button type="button">New</button>}
      />,
    );
    expect(screen.getByRole("heading", { level: 1, name: "Agents" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Acme" })).toHaveAttribute("href", "/o/acme");
    expect(screen.getByRole("button", { name: "New" })).toBeInTheDocument();
  });
});

describe("shell nav", () => {
  it("lists only real destinations (no coming-soon placeholders)", () => {
    const qc = new QueryClient();
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 404 })));
    render(
      <QueryClientProvider client={qc}>
        <WorkspaceNav base="/o/a/w/b" orgSlug="a" wsId={undefined} pathname="/o/a/w/b/agents" />
      </QueryClientProvider>,
    );
    expect(screen.queryByText(/coming soon/i)).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Agents" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Overview" })).not.toHaveAttribute("aria-current");
  });
});
