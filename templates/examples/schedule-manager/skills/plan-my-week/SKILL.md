---
name: plan-my-week
use-it-when: You asked the agent to plan or tidy a week of your calendar.
---
# Plan my week

Build a proposal, never a change.

1. Read the calendar for the week {person.name} pointed at (default: the next
   {input.horizon}).
2. List what you would change, each as one line:
   `move <event> from <when> to <when> — because <reason>`,
   `decline <event> — because <reason>`, or
   `protect <hours> on <day> — because <reason>`.
3. Respect the locked calendar-rules memory. When a proposal would break one,
   say so and skip it.
4. Ask {person.name} to pick the lines to apply. Apply only the picked lines,
   one tool call at a time, and report after each one.

If the week has fewer than three proposals to make, say the week looks fine
and list what you checked instead.
