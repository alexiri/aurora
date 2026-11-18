import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { LlmKeyForm } from "@/components/credentials/llm-form";
import {
  buildValue,
  harnessHint,
  keyReasonMessage,
  modeLabel,
  shapeOf,
  trustPolicy,
  valueComplete,
} from "@/components/credentials/llm-key-modes";
import { ApiError } from "@/lib/api/client";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
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

const EXTERNAL_ID = "ext-7f3a9c1d";
const org = { id: "o1", slug: "acme", name: "Acme", external_id: EXTERNAL_ID };

const providers = {
  providers: [
    {
      id: "bedrock",
      display_name: "Amazon Bedrock",
      dialects: [],
      auth_modes: ["aws_sigv4", "aws_assume_role", "api_key"],
      auth_mode_details: [
        { id: "aws_sigv4", value_shape: "aws_sigv4", recommended: false },
        { id: "aws_assume_role", value_shape: "aws_assume_role", recommended: true },
        { id: "api_key", value_shape: "string", recommended: false },
      ],
    },
    {
      id: "azure_openai",
      display_name: "Azure OpenAI",
      dialects: [],
      auth_modes: ["api_key", "azure_entra"],
      auth_mode_details: [
        { id: "api_key", value_shape: "string", recommended: false },
        { id: "azure_entra", value_shape: "azure_entra", recommended: true },
      ],
    },
    {
      id: "vertex",
      display_name: "Google Vertex AI",
      dialects: [],
      auth_modes: ["gcp_service_account"],
      auth_mode_details: [{ id: "gcp_service_account", value_shape: "gcp_service_account", recommended: true }],
    },
  ],
  endpoints: [
    {
      id: "bed",
      provider: "bedrock",
      display_name: "Bedrock EU",
      base_url: "https://bedrock-runtime.eu-central-1.amazonaws.com",
      dialects: ["anthropic_bedrock"],
      permitted: true,
      key_scopes: ["organization"],
      auth_modes: ["aws_sigv4", "aws_assume_role", "api_key"],
      recommended_auth_mode: "aws_assume_role",
      harnesses: ["claude_code"],
    },
    {
      id: "az",
      provider: "azure_openai",
      display_name: "Azure",
      base_url: "https://res.openai.azure.com",
      dialects: ["openai_chat"],
      permitted: true,
      key_scopes: ["organization"],
      auth_modes: ["api_key", "azure_entra"],
      recommended_auth_mode: "azure_entra",
      harnesses: [],
    },
    {
      id: "vx",
      provider: "vertex",
      display_name: "Vertex",
      base_url: "https://aiplatform.googleapis.com",
      dialects: ["anthropic_vertex"],
      permitted: true,
      key_scopes: ["organization"],
      auth_modes: ["gcp_service_account"],
      recommended_auth_mode: "gcp_service_account",
      harnesses: ["claude_code"],
    },
  ],
};

function open(extra: Record<string, Handler> = {}) {
  const fn = mockFetch({
    "GET /v1/llm/providers": () => json(200, providers),
    "GET /v1/orgs/o1": () => json(200, org),
    ...extra,
  });
  wrap(<LlmKeyForm scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
  return fn;
}

async function pick(endpoint: string) {
  await screen.findByText(/Bedrock EU/);
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "k" } });
  fireEvent.change(screen.getByLabelText("Provider endpoint"), { target: { value: endpoint } });
}

describe("LLM key form per auth mode (C-811)", () => {
  it("offers the modes as a radio group with the recommended one first, and shows the role fields, the external id and the trust policy", async () => {
    open();
    await pick("bed");
    const group = screen.getByRole("radiogroup", { name: "How Aurora signs in to the provider" });
    const radios = within(group).getAllByRole("radio");
    expect(radios.map((r) => (r.closest("label") as HTMLElement).textContent)).toEqual([
      expect.stringContaining("Assume a role"),
      expect.stringContaining("Access keys"),
      expect.stringContaining("Bedrock API key"),
    ]);
    expect(within(group).getByText("Recommended")).toBeTruthy();
    expect((radios[0] as HTMLInputElement).checked).toBe(true);
    // Assume-role fields, the org's external id and a trust policy that requires it, with Copy.
    expect(screen.getByLabelText("Role ARN")).toBeTruthy();
    expect(screen.getByLabelText("Source access key id")).toBeTruthy();
    expect((screen.getByLabelText("Source secret access key") as HTMLInputElement).type).toBe("password");
    expect(screen.getByLabelText("Session length in seconds (optional)")).toBeTruthy();
    expect(await screen.findByText(EXTERNAL_ID)).toBeTruthy();
    const policy = screen.getByLabelText("the trust policy");
    expect(policy.textContent).toContain(`"sts:ExternalId": "${EXTERNAL_ID}"`);
    expect(screen.getByRole("button", { name: "Copy the trust policy" })).toBeTruthy();
    // Another mode swaps the fields.
    fireEvent.click(screen.getByLabelText(/Access keys/));
    expect(screen.queryByLabelText("Role ARN")).toBeNull();
    expect(screen.queryByLabelText("the trust policy")).toBeNull();
    expect(screen.getByLabelText("Access key id")).toBeTruthy();
    expect(screen.getByLabelText("Session token (optional)")).toBeTruthy();
  });

  it("sends the assume-role value as the structured JSON the API expects and clears every field afterwards", async () => {
    let body: { llm: { endpoint_id: string; auth_mode?: string }; value: string; kind: string } | null = null;
    open({
      "POST /v1/orgs/o1/secrets": async (req) => {
        body = await req.clone().json();
        return json(422, { error: { code: "llm_key_invalid", message: "m", details: { reason: "assume_role_denied" } } });
      },
    });
    await pick("bed");
    fireEvent.change(screen.getByLabelText("Role ARN"), { target: { value: "arn:aws:iam::123456789012:role/aurora" } });
    fireEvent.change(screen.getByLabelText("Source access key id"), { target: { value: "AKIAEXAMPLE" } });
    fireEvent.change(screen.getByLabelText("Source secret access key"), { target: { value: "s3cr3t" } });
    fireEvent.change(screen.getByLabelText("Session length in seconds (optional)"), { target: { value: "1800" } });
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect(body!.llm).toEqual({ endpoint_id: "bed", auth_mode: "aws_assume_role" });
    expect(JSON.parse(body!.value)).toEqual({
      role_arn: "arn:aws:iam::123456789012:role/aurora",
      access_key_id: "AKIAEXAMPLE",
      secret_access_key: "s3cr3t",
      session_duration_secs: 1800,
    });
    // C-801 details.reason in plain language, never the raw code.
    expect(await screen.findByText(/trust policy: it must allow the source user and require this organization's external id/)).toBeTruthy();
    expect((screen.getByLabelText("Source secret access key") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText("Source access key id") as HTMLInputElement).value).toBe("");
  });

  it("shows the Entra fields for Azure, the harness hint and the no-harness notice", async () => {
    open();
    await pick("az");
    expect(screen.getByText(/No enabled harness speaks this endpoint's API/)).toBeTruthy();
    expect(screen.getByText("Models are configured by your administrator.")).toBeTruthy();
    const group = screen.getByRole("radiogroup", { name: "How Aurora signs in to the provider" });
    expect(within(group).getAllByRole("radio").map((r) => (r.closest("label") as HTMLElement).textContent)[0]).toContain(
      "Microsoft Entra application",
    );
    expect(screen.getByLabelText("Tenant id")).toBeTruthy();
    expect(screen.getByLabelText("Client id")).toBeTruthy();
    expect((screen.getByLabelText("Client secret") as HTMLInputElement).type).toBe("password");
    // The other mode is the plain resource key.
    fireEvent.click(within(group).getByLabelText(/Resource key/));
    expect(screen.getByLabelText("API key")).toBeTruthy();
    // An endpoint that serves a harness says so in the picker.
    expect(screen.getByRole("option", { name: /Bedrock EU \(bedrock\) — Works with Claude Code/ })).toBeTruthy();
  });

  it("takes a GCP service account key file or pasted JSON, and sends it as the value", async () => {
    let body: { value: string; llm: { auth_mode?: string } } | null = null;
    open({
      "POST /v1/orgs/o1/secrets": async (req) => {
        body = await req.clone().json();
        return json(500, { error: { code: "internal", message: "stop" } });
      },
    });
    await pick("vx");
    // One mode only: no radio group.
    expect(screen.queryByRole("radiogroup")).toBeNull();
    const file = new File([JSON.stringify({ type: "service_account", project_id: "p" })], "key.json", { type: "application/json" });
    fireEvent.change(screen.getByLabelText("Choose a key file"), { target: { files: [file] } });
    await waitFor(() => expect((screen.getByLabelText("Service account key (JSON)") as HTMLTextAreaElement).value).toContain("service_account"));
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect(JSON.parse(body!.value)).toEqual({ type: "service_account", project_id: "p" });
    expect(body!.llm.auth_mode).toBeUndefined();
  });

  it("refuses an incomplete structured value before any request", async () => {
    const fn = open();
    await pick("bed");
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    expect(await screen.findByText("Fill in every required field.")).toBeTruthy();
    expect(fn.mock.calls.some(([r]) => (r as Request).method === "POST")).toBe(false);
  });
});

describe("LLM key modes (pure helpers)", () => {
  it("builds the C-801 value of each shape", () => {
    expect(buildValue("string", "sk-1", {})).toBe("sk-1");
    expect(buildValue("gcp_service_account", "{}", {})).toBe("{}");
    expect(JSON.parse(buildValue("aws_sigv4", "", { access_key_id: " A ", secret_access_key: "S", session_token: "" }))).toEqual({
      access_key_id: "A",
      secret_access_key: "S",
    });
    expect(JSON.parse(buildValue("azure_entra", "", { tenant_id: "t", client_id: "c", client_secret: "s" }))).toEqual({
      tenant_id: "t",
      client_id: "c",
      client_secret: "s",
    });
  });

  it("knows what is complete, and which shape a mode has", () => {
    expect(valueComplete("aws_sigv4", "", { access_key_id: "a" })).toBe(false);
    expect(valueComplete("aws_sigv4", "", { access_key_id: "a", secret_access_key: "b" })).toBe(true);
    expect(valueComplete("string", " ", {})).toBe(false);
    expect(shapeOf([{ id: "x", value_shape: "azure_entra" }], "x")).toBe("azure_entra");
    expect(shapeOf(undefined, "api_key")).toBe("string");
  });

  it("names modes and harnesses in words", () => {
    expect(modeLabel("api_key", "azure_openai")).toBe("Resource key");
    expect(modeLabel("api_key", "bedrock")).toBe("Bedrock API key");
    expect(modeLabel("aws_assume_role", "bedrock")).toBe("Assume a role");
    expect(harnessHint(["claude_code", "codex"])).toBe("Works with Claude Code, Codex");
    // (P5-HARN, C-1016) The API's ids are the registry's (`claude-code`), and the added harnesses have names.
    expect(harnessHint(["claude-code", "pi", "goose", "opencode"])).toBe("Works with Claude Code, Pi, goose, OpenCode");
    expect(harnessHint([])).toBe("");
    expect(JSON.parse(trustPolicy("abc")).Statement[0].Condition.StringEquals["sts:ExternalId"]).toBe("abc");
  });

  it("words every C-801 reason and ignores other errors", () => {
    for (const r of [
      "auth_mode_unsupported",
      "missing_field",
      "unknown_field",
      "role_arn_invalid",
      "partition_mismatch",
      "duration_out_of_range",
      "tenant_not_specific",
      "token_uri_not_google",
      "not_service_account",
      "assume_role_denied",
      "mint_rejected",
      "provider_rejected",
    ]) {
      const m = keyReasonMessage(new ApiError("llm_key_invalid", "m", { reason: r }, 422));
      expect(m, r).toBeTruthy();
      expect(m).not.toContain("_");
    }
    expect(keyReasonMessage(new ApiError("llm_key_invalid", "m", { reason: "new_one" }, 422))).toBeUndefined();
    expect(keyReasonMessage(new ApiError("name_taken", "m", { reason: "missing_field" }, 409))).toBeUndefined();
    expect(keyReasonMessage(new Error("x"))).toBeUndefined();
  });
});
