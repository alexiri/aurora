---
key: escalation-policy
updated: 2026-11-20
locked: true
---
Escalation policy {person.name} set and asked to keep:

- Degraded for over 30 minutes → propose filing an issue in
  {input.jira_project}, even if nobody asked.
- A repeated alert on the same service within 24 hours → link the previous
  issue instead of proposing a new one.
- Never propose restarting or redeploying anything; that is a person's call.
