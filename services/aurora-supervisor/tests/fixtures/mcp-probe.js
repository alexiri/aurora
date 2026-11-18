// A stdio MCP server for the harness conformance suites (docs/12 §Adapters pi, goose and opencode,
// D-301). It exposes one tool, `ping`; each call appends the process ancestry of this server (pid,
// parent pid and command line, from this process up to the top) to the file named by
// MCP_PROBE_LOG as one JSON line, so a test can prove that platform-tool calls come from a
// descendant of the harness process the supervisor started.
const fs = require("fs");
const readline = require("readline");

function chain() {
  const out = [];
  let pid = process.pid;
  for (let i = 0; i < 64 && pid > 0; i++) {
    let stat, cmd;
    try {
      stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
      cmd = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0").join(" ").trim();
    } catch {
      break;
    }
    // `pid (comm) state ppid ...`: comm may contain spaces and parentheses.
    const rest = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
    const ppid = parseInt(rest[1], 10);
    out.push({ pid, ppid, cmd });
    if (ppid === pid) break;
    pid = ppid;
  }
  return out;
}

const send = (o) => process.stdout.write(JSON.stringify(o) + "\n");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  let m;
  try {
    m = JSON.parse(line);
  } catch {
    return;
  }
  if (m.id === undefined) return; // a notification
  switch (m.method) {
    case "initialize":
      send({
        jsonrpc: "2.0",
        id: m.id,
        result: {
          protocolVersion: (m.params && m.params.protocolVersion) || "2024-11-05",
          capabilities: { tools: {} },
          serverInfo: { name: "aurora-probe", version: "0" },
        },
      });
      break;
    case "tools/list":
      send({
        jsonrpc: "2.0",
        id: m.id,
        result: {
          tools: [
            {
              name: "ping",
              description: "Answers pong and records who called it.",
              inputSchema: { type: "object", properties: {} },
            },
          ],
        },
      });
      break;
    case "tools/call":
      if (process.env.MCP_PROBE_LOG) {
        fs.appendFileSync(process.env.MCP_PROBE_LOG, JSON.stringify({ chain: chain() }) + "\n");
      }
      send({ jsonrpc: "2.0", id: m.id, result: { content: [{ type: "text", text: "pong" }] } });
      break;
    default:
      send({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "method not found" } });
  }
});
