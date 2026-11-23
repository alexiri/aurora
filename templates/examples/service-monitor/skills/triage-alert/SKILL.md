---
name: triage-alert
use-it-when: A dashboard or an alert needs to be judged and summarized before anyone decides anything.
---
# Triage an alert

1. Read the dashboard(s) named in the alert and the PromQL queries behind the
   panels that matter. Run the deciding query yourself if the dashboard does
   not answer it.
2. Check whether the problem is already known: search Jira project
   {input.jira_project} for open issues about the service, and check the
   GitLab pipeline and recent commits when the alert started near a
   deployment.
3. Write the summary block with the helper script:

   ```sh
   .aurora/helpers/alert-summary.sh < alert.json
   ```

   It takes the alert's JSON on stdin and prints the four summary lines
   (what, when, evidence, known?) in the report format. Paste its output and
   add at most three sentences of context.
4. End with exactly one recommendation: watch, investigate, or file an issue.
   Cite the numbers behind it.

The escalation-policy memory decides when to propose an issue without being
asked.
