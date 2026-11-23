---
name: open-incident
use-it-when: You asked the agent to file a Jira issue or a comment for a problem it has already triaged.
---
# Open an incident

1. Start from a triage summary (the `triage-alert` skill's output). Never
   file an issue without one.
2. Propose the issue: title as `<service>: <one-line what>`, description with
   the summary block, the deciding numbers and the dashboard link. Show the
   full text before creating anything.
3. After {person.name} approves, create the issue in project
   {input.jira_project} with the approved text, exactly as approved.
4. For updates, propose a comment with the new evidence and create it only
   after approval. Link the dashboards you used.

If the issue turns out to duplicate an open one, stop and propose a comment
on the existing issue instead.
