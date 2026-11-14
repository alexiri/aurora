import { DEFAULT, MINE, type Ev, type Fixture } from "./chat-mock";

/** One agent reply that uses every Markdown element FIX-137 renders (a wide table included). */
export const EVERYTHING = `# Weekly report

Some **bold**, *italic*, ~~struck~~ and \`inline code\`, a bare link https://example.com/very/long/path/that/keeps/going/and/going/and/going/and/going and [the docs](https://docs.example/a). Footnote[^1] and :tada:.

> [!NOTE]
> Heads up.

> [!WARNING]
> Careful now.

- [x] done
- [ ] todo
  - nested item

1. first
2. second

| Service | Region | Owner | Latency p50 (ms) | Latency p99 (ms) | Error rate | Notes |
|:--|:-:|--:|--:|--:|--:|:--|
| importer-service-with-a-long-name | eu-west-1 | platform-team@example.com | 120 | 940 | 0.12% | retry budget exhausted twice during the nightly run |
| gateway | us-east-1 | edge-team@example.com | 18 | 77 | 0.01% | fine |

\`\`\`ts
const answer: number = 42; // a long line that has to scroll inside its own box rather than push the page sideways past the edge
\`\`\`

\`\`\`mermaid
graph TD; A-->B;
\`\`\`

Inline math $a^2 + b^2 = c^2$ and a block:

$$\\int_0^1 x\\,dx = \\tfrac12$$

<details><summary>More</summary>

Hidden body with <kbd>Ctrl</kbd>+<kbd>C</kbd>, H<sub>2</sub>O and x<sup>2</sup>.

</details>

![remote](https://evil.example/leak.png)

---

[^1]: The note.`;

const at = (s: number) => new Date(Date.now() - s * 1000).toISOString();
export const MARKDOWN_FIXTURE: Fixture = {
  ...DEFAULT,
  events: {
    [MINE.id]: [
      { seq: 1, type: "turn.queued", ts: at(120), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "Give me the report.\nWith **formatting**.", attachments: [] } },
      { seq: 2, type: "text", ts: at(100), payload: { turn_id: "t1", message_id: "m2", text: EVERYTHING } },
      { seq: 3, type: "turn.finished", ts: at(99), payload: { turn_id: "t1", status: "done" } },
    ] satisfies Ev[],
  },
};
