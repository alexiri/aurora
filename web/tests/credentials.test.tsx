import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { LlmKeyForm } from "@/components/credentials/llm-form";
import { ERROR_MESSAGES } from "@/lib/api/errors";
import { buildGenericCreate, buildRecipe, EMPTY_RECIPE_FIELDS, type GenericFormState } from "@/lib/api/secrets";
import { wrap } from "./support/render";
import { json } from "./support/api";

function mockFetch(routes: Record<string, (req: Request) => Response | Promise<Response>>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const h = routes[`${req.method} ${new URL(req.url).pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

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
    fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "ok" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    expect(await screen.findByText(ERROR_MESSAGES.llm_key_wrong_organization!)).toBeTruthy();
    expect((screen.getByLabelText("API key") as HTMLInputElement).value).toBe("");
  });
});
