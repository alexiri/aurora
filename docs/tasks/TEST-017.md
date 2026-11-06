# TEST-017: VidaiMock behind real LiteLLM; live `claude-code` conformance

## Goal
Implement decision **I-30**: **VidaiMock** (`github.com/vidaiUK/VidaiMock`, pinned image digest) with Aurora-owned provider config, templates and scenario files, behind the real LiteLLM proxy in a compose profile, and make the live `claude-code` conformance suite and a hosted e2e turn with the real CLI pass against it.

## Linked requirements
- `docs/19` M5 acceptance: "live conformance passes against a configured LiteLLM endpoint".
- `docs/12-supervisor-harness.md` §Adapter `claude-code`, §Provider refusal classification; `docs/08-secrets-credentials.md` §6 (LiteLLM provider, endpoints), §11 governance (endpoint + key origin); `docs/06-gateway.md` (LLM injection lane).
- Existing suite: `services/aurora-supervisor/tests/conformance.rs`. The `LIVE` profile uses plain-language prompts, env `AURORA_LIVE_CLAUDE=1`, `ANTHROPIC_BASE_URL`, `AURORA_LIVE_MODEL`, `AURORA_LIVE_MCP_BRIDGE`, `AURORA_CLAUDE_BIN`. Claude Code CLI 2.1.319 is pinned in `images/sandbox/Containerfile`. Install the same version locally for the supervisor-level run (`npm i -g @anthropic-ai/claude-code@2.1.319` into a scratch prefix, or run the suite inside the sandbox image), and report how you did it.

## Scope
### In scope
1. **VidaiMock configuration** (`deploy/compose/vidaimock/`):
   - First read the upstream README, `config/providers/anthropic*.yaml`, `config/templates/anthropic/**`, and `mkdocs/docs/{templating,streaming,chaos-and-errors,tool-calling,agentic-testing}.md` and `examples/` at the pinned release.
   - Write Aurora's own provider YAML(s), Tera templates and **scenario files** (`<id>.sse` / `.body` / `.status`, `:vidai-delay`) covering every `LIVE` conformance case: "pong", the long slow count, a tool call **with real arguments** through the MCP bridge followed by a final answer after `tool_result`, a steerable case, and a refusal/overload case (429/529 → refusal classification). Also cover the hosted e2e prompts.
   - Select scenarios deterministically. Preferred: one LiteLLM model alias per scenario (`scn-<id>` upstream model names), with the conformance suite in mock mode choosing the per-case model through a new optional `AURORA_LIVE_SCENARIO_MODELS` mapping. Alternatively, Tera rules on the request.
   - Known upstream limits to design around: the bundled template emits tool input `{}` and stops calling tools once any `tool_result` exists; Tera indexing into mixed-type content arrays is documented as unreliable. Prefer scenario playback over content-sniffing templates.
   - Pin the image **by digest** and record the version and digest in `deploy/compose/vidaimock/README.md`, with the license (Apache-2.0).
   - **Do not build a first-party mock.** If a case truly cannot be expressed, stop and report it through Orca `ask` with specifics.
2. **Compose profile `llm-mock`** (`deploy/compose/compose.yaml`):
   - `vidaimock` service (pinned digest, config mounted read-only), plus a `litellm` service with a pinned image tag/digest and `deploy/compose/litellm.yaml`, whose `model_list` maps e.g. `claude-mock` (and an Anthropic-style id the CLI accepts) to `anthropic/<id>` with `api_base: http://vidaimock:<port>`.
   - Provide a dummy master key via env and document it as dev-only.
   - Add an `[[llm.endpoints]]` entry in `deploy/compose/aurora.toml` (or a profile-specific overlay) so the gateway's LiteLLM provider points at `litellm:4000`.
   - Governance must permit it. Use the dev governance example in docs/15.
3. **Runs** (scripts under `scripts/e2e/llm.sh`):
   - (a) The supervisor conformance suite in live mode against LiteLLM → mock: `AURORA_LIVE_CLAUDE=1 ANTHROPIC_BASE_URL=http://127.0.0.1:<litellm>`. Directly to LiteLLM is acceptable here because the supervisor suite has no gateway, so say so in the report.
   - (b) A hosted e2e scenario in `tests/e2e-hosted` (new test, gated by `AURORA_TEST_LLM=1`): an agent with the `claude-code` harness and an LLM key for the LiteLLM endpoint gets a chat message, and the answer arrives through the transcript. This proves the gateway LLM lane + context credentials + the real CLI in the sandbox image. Reuse the TEST-013 harness and the podman sandbox image.
4. `docs/operator/llm-endpoints.md` and `docs/15` dev section: how to run with the mock, and that it is not a model.

### Out of scope
- Real-model runs (manual, optional). Changing adapter behaviour, unless a real bug is found; then fix it minimally and report.

## Constraints / file ownership
- You own: `deploy/compose/vidaimock/**`, `deploy/compose/{compose.yaml,litellm.yaml,aurora.toml}` (profile additions only), `scripts/e2e/llm.sh`, the new e2e test file in `tests/e2e-hosted/tests/`, and the docs lines above. The `AURORA_LIVE_SCENARIO_MODELS` addition to `services/aurora-supervisor/tests/conformance.rs` (mock mode only; the real-model path must stay unchanged). Make minimal fixes in `services/aurora-supervisor/src/adapters/claude_code/**` only if a real defect shows up.
- Pull only pinned images. Don't publish anything. No real API keys anywhere.
- Disk: the sandbox image build needs about 15 GB free; check `df` first.

## Acceptance criteria
- [ ] The scenario files are exercised by a small smoke script (curl through LiteLLM; streaming tool_use with non-empty input, then tool_result → final text; 529 case).
- [ ] Live conformance: every `LIVE` case passes against LiteLLM → mock (paste the summary).
- [ ] `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh` passes the hosted claude-code scenario.
- [ ] fmt, clippy `-D warnings`, `cargo deny check` and guards pass.

## Dependencies
- Blocked by: SUP-004, RUN-003, TEST-013 (done); FIX-002 (in progress; needed for app env, not for LLM). Unblocks: M5 acceptance, TEST-014.

## Suggested agent/model
Sonnet, own worktree.
