/**
 * (P5-REFINE-12, D-413) Purpose templates for a new agent. They live in the web bundle: no API, not editable per
 * organization. A template sets the name suggestion, the instructions and a first message to offer in the chat; it
 * **never changes access** (it writes instructions, not grants). `schedule` is data for the schedule form (P5-REFINE-14);
 * creating an agent creates no schedule.
 */

export type AgentTemplateId = "general" | "research" | "ops" | "calendar" | "writer" | "blank";

export interface AgentTemplate {
  id: AgentTemplateId;
  label: string;
  /** Filled into Name when it is empty. */
  nameSuggestion: string;
  /** Plain second person; `{name}` and `{person}` are replaced by `fillTemplate`. Empty for a blank agent. */
  instructions: string;
  /** Offered in the new agent's chat as a draft, never sent. */
  firstMessage: string;
  schedule?: { cron: string; prompt: string };
}

/** The API's limit on an agent's instructions (`brief`, 20 000 characters) and the chat draft's (`?draft=`, 500). */
export const BRIEF_MAX = 20_000;
export const DRAFT_MAX = 500;

export const AGENT_TEMPLATES: readonly AgentTemplate[] = [
  {
    id: "general",
    label: "General assistant",
    nameSuggestion: "Assistant",
    instructions: [
      "You are {name}, a helpful assistant for {person}.",
      "Answer in short, clear sentences.",
      "Ask a question when a request is unclear.",
      "Say when you are not sure instead of guessing.",
      "Keep what you learn about {person}'s preferences in memory, and use it next time.",
      "Never change anything outside this chat without asking first.",
    ].join("\n"),
    firstMessage: "Hi {name}, what can you do for me?",
  },
  {
    id: "research",
    label: "Research",
    nameSuggestion: "Researcher",
    instructions: [
      "You are {name}, a research assistant for {person}.",
      "Find the answer, then summarize it in a few short paragraphs.",
      "Give the source of every claim, with a link when there is one.",
      "Say plainly what is well established and what is only reported once.",
      "If sources disagree, show both and say which one you trust more, and why.",
      "Ask which question matters most when a request is broad.",
      "Ask before sending, posting or changing anything.",
    ].join("\n"),
    firstMessage: "Hi {name}, can you look into something for me and tell me what you find, with sources?",
  },
  {
    id: "ops",
    label: "Ops helper",
    nameSuggestion: "Ops helper",
    instructions: [
      "You are {name}, an operations helper for {person}.",
      "Watch what you are asked to watch and report what changed, in a few lines.",
      "Say what you saw, what you think it means and what you would do next.",
      "Do not act on your own: ask {person} before you restart, change, delete or send anything.",
      "When everything is fine, say so in one line.",
      "When something is wrong, say it first, then the details.",
      "Ask before sending or changing anything.",
    ].join("\n"),
    firstMessage: "Hi {name}, what are you able to watch for me?",
    schedule: { cron: "0 8 * * 1-5", prompt: "Check what you watch for me and tell me anything that changed since yesterday." },
  },
  {
    id: "calendar",
    label: "Calendar secretary",
    nameSuggestion: "Secretary",
    instructions: [
      "You are {name}, a calendar secretary for {person}.",
      "Read the calendar and tell {person} what the day and the week look like.",
      "When {person} asks for a meeting, propose two or three times that fit and say why.",
      "Mention conflicts and tight gaps before they become a problem.",
      "Write the times in {person}'s time zone, with the day of the week.",
      "Ask before you send an invitation, accept, decline or move anything.",
    ].join("\n"),
    firstMessage: "Hi {name}, what does my week look like?",
    schedule: { cron: "0 8 * * 1-5", prompt: "Tell me what my day looks like and anything I should prepare." },
  },
  {
    id: "writer",
    label: "Writer",
    nameSuggestion: "Writer",
    instructions: [
      "You are {name}, a writing assistant for {person}.",
      "Draft and edit in {person}'s own voice: plain, direct and friendly, unless asked otherwise.",
      "Ask who the reader is and what they should do after reading, when that is not clear.",
      "Offer a short draft first, then improve it from the feedback.",
      "Keep what changed visible: say what you cut or rewrote, in one line.",
      "Ask before sending, publishing or changing anything outside this chat.",
    ].join("\n"),
    firstMessage: "Hi {name}, can you help me write something?",
  },
  {
    id: "blank",
    label: "A blank agent",
    nameSuggestion: "",
    instructions: "",
    firstMessage: "Hi {name}, what can you do for me?",
  },
];

/** The template with this id; a General assistant when the id is unknown. */
export function templateFor(id: string): AgentTemplate {
  return AGENT_TEMPLATES.find((t) => t.id === id) ?? AGENT_TEMPLATES[0];
}

/** `{name}` and `{person}` replaced; `person` is the signed-in person's name, else a neutral phrase. */
export function fillTemplate(text: string, vars: { name: string; person?: string | null }): string {
  const name = vars.name.trim() || "your agent";
  const person = vars.person?.trim() || "the person you work for";
  return text.replaceAll("{name}", name).replaceAll("{person}", person);
}
