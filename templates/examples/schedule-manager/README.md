# Schedule manager

## What it does

The agent reads the Indico categories you follow and your calendar, and keeps
a short, useful picture of your day and your week:

- **Every weekday morning** it posts an agenda: your calendar for the day,
  Indico events starting today or tomorrow, and up to three things to watch
  out for (clashes, long meeting blocks, nearby deadlines).
- **On request** it plans or tidies a week: it proposes moves, declinations
  and protected work time, and makes a change only after you say yes.

It does **not** write on its own. Booking, moving or declining anything
always needs your explicit approval. It does not read or write anything else:
no email, no documents, no chat.

## Who it's for

People whose week is split between internal meetings and events tracked in
Indico — seminar and conference organizers, group assistants, scientists who
run a series. You need to be connected to Indico and to one of the supported
calendar apps.

## What it needs

- **Indico** (read): the agent reads the categories you give it by id.
- **A calendar app** — Microsoft 365, CalDAV or Google Workspace, whichever
  your organization offers (ask level): it reads your calendar and proposes
  changes, and asks before every change.

## What it does on its own

A schedule **"Morning agenda"** runs every weekday morning and writes the
agenda into its own conversation. It never messages anyone first.

## Adapting it

After creation you can freely edit the instructions, add or remove skills and
memories, and change the schedule's time or prompt — the template is only a
starting point. If you stop following a category, just remove its id from the
instructions.
