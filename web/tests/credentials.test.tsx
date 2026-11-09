import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { GenericSecretForm } from "@/components/credentials/generic-form";
import { LlmKeyForm } from "@/components/credentials/llm-form";
import { ERROR_MESSAGES } from "@/lib/api/errors";
import { buildGenericCreate, buildRecipe, EMPTY_RECIPE_FIELDS, type GenericFormState } from "@/lib/api/secrets";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function mockFetch(routes: Record<string, (req: Request) => Response | Promise<Response>>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const h = routes[`${req.method} ${new URL(req.url).pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const F = EMPTY_RECIPE_FIELDS;

describe("recipe mapping", () => {
  it("maps each kind", () => {
    expect(buildRecipe("bearer", F)).toEqual({ kind: "bearer" });
    expect(buildRecipe("header", { ...F, name: "X-Key", format: "Token {value}" })).toEqual({
      kind: "header", name: "X-Key", format: "Token {value}",
    });
    expect(buildRecipe("query", { ...F, name: "k" })).toEqual({ kind: "query", name: "k" });
    expect(buildRecipe("basic", { ...F, username: "u" })).toEqual({ kind: "basic", username: "u" });
    expect(buildRecipe("path_template", { ...F, prefix: "/bot" })).toEqual({ kind: "path_template", prefix: "/bot" });
    expect(buildRecipe("path_regex", { ...F, pattern: "a", replacement: "b" })).toEqual({
      kind: "path_regex", pattern: "a", replacement: "b",
    });
  });

  it("builds value and reference create bodies", () => {
    const s: GenericFormState = {
      name: " n ", host: "api.x.com", path: "", kind: "bearer", fields: F, source: "value", value: "v",
      ref: { store_id: "vault", path: "p", field: "" },
    };
    expect(buildGenericCreate(s)).toEqual({
      name: "n", kind: "generic", host_pattern: "api.x.com", injection: { kind: "bearer" }, value: "v",
    });
    expect(buildGenericCreate({ ...s, source: "reference", path: "/v1" })).toEqual({
      name: "n", kind: "generic", host_pattern: "api.x.com", path_pattern: "/v1", injection: { kind: "bearer" },
      reference: { store_id: "vault", path: "p" },
    });
  });
});

describe("generic form", () => {
  const stores = (references: boolean) => ({
    items: [{ id: "managed", kind: "managed", capabilities: { managed: true, references } }],
  });

  it("hides reference fields when no store supports references", async () => {
    mockFetch({ "GET /v1/secret-stores": () => json(200, stores(false)) });
    wrap(<GenericSecretForm scope={{ kind: "workspace", id: "w1" }} orgId="o1" />);
    await screen.findByLabelText("Value");
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByLabelText("Value source")).toBeNull();
  });

  it("offers reference mode when available", async () => {
    mockFetch({ "GET /v1/secret-stores": () => json(200, stores(true)) });
    wrap(<GenericSecretForm scope={{ kind: "workspace", id: "w1" }} orgId="o1" />);
    expect(await screen.findByLabelText("Value source")).toBeTruthy();
  });

  it("posts the body and clears the value after submit", async () => {
    let body: unknown;
    mockFetch({
      "GET /v1/secret-stores": () => json(200, stores(false)),
      "POST /v1/workspaces/w1/secrets": async (req) => {
        body = await req.json();
        return json(409, { error: { code: "name_taken", message: "x" } });
      },
    });
    wrap(<GenericSecretForm scope={{ kind: "workspace", id: "w1" }} orgId="o1" />);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "gh" } });
    fireEvent.change(screen.getByLabelText("Host pattern"), { target: { value: "api.github.com" } });
    const value = screen.getByLabelText("Value") as HTMLInputElement;
    expect(value.type).toBe("password");
    fireEvent.change(value, { target: { value: "s3cret" } });
    fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
    await waitFor(() => expect(body).toBeTruthy());
    expect(body).toMatchObject({ name: "gh", kind: "generic", value: "s3cret", injection: { kind: "bearer" } });
    await waitFor(() => expect((screen.getByLabelText("Value") as HTMLInputElement).value).toBe(""));
    expect(await screen.findByText(ERROR_MESSAGES.name_taken!)).toBeTruthy();
  });
});

describe("llm form", () => {
  const providers = {
    providers: [{ id: "openai", display_name: "OpenAI", dialects: [], auth_modes: ["api_key"] }],
    endpoints: [
      { id: "ok", provider: "openai", display_name: "OpenAI API", base_url: "https://x", dialects: [], permitted: true, key_scopes: ["organization"] },
      { id: "no", provider: "openai", display_name: "Forbidden", base_url: "https://y", dialects: [], permitted: false },
    ],
  };

  it("lists only permitted endpoints and renders governance errors", async () => {
    mockFetch({
      "GET /v1/llm/providers": () => json(200, providers),
      "POST /v1/orgs/o1/secrets": () => json(422, { error: { code: "llm_key_wrong_organization", message: "m" } }),
    });
    wrap(<LlmKeyForm scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    await screen.findByText(/OpenAI API/);
    expect(screen.queryByText(/Forbidden/)).toBeNull();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "k" } });
    fireEvent.change(screen.getByLabelText("Provider endpoint"), { target: { value: "ok" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    expect(await screen.findByText(ERROR_MESSAGES.llm_key_wrong_organization!)).toBeTruthy();
    expect((screen.getByLabelText("API key") as HTMLInputElement).value).toBe("");
  });
});
