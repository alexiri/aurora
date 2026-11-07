#!/usr/bin/env node
// Minimal MCP stdio server named `aurora` with one tool, `memory_save`, for the supervisor-level
// live conformance run (scripts/e2e/llm.sh). The real bridge (`aurora-supervisor mcp-bridge`) needs a
// running supervisor socket; the hosted e2e uses the real one. Dependency-free; newline-delimited JSON-RPC.
import readline from "node:readline";

const TOOL = {
  name: "memory_save",
  description: "Save a short note to the agent's memory.",
  inputSchema: {
    type: "object",
    additionalProperties: false,
    properties: { key: { type: "string" }, content: { type: "string", description: "Markdown body." } },
    required: ["key", "content"],
  },
};
const send = (m) => process.stdout.write(JSON.stringify(m) + "\n");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id === undefined) return; // notification
  const ok = (result) => send({ jsonrpc: "2.0", id: msg.id, result });
  switch (msg.method) {
    case "initialize":
      return ok({
        protocolVersion: msg.params?.protocolVersion ?? "2025-06-18",
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: "aurora", version: "stub" },
      });
    case "ping": return ok({});
    case "tools/list": return ok({ tools: [TOOL] });
    case "tools/call": {
      const { key, content } = msg.params?.arguments ?? {};
      if (msg.params?.name !== "memory_save" || typeof key !== "string" || typeof content !== "string" || key === "") {
        return ok({ content: [{ type: "text", text: "invalid arguments: key and content are required" }], isError: true });
      }
      return ok({ content: [{ type: "text", text: JSON.stringify({ key, revision: 1, created: true }) }], isError: false });
    }
    default:
      return send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "method not found" } });
  }
});
