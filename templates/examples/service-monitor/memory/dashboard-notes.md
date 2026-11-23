---
key: dashboard-notes
updated: 2026-11-20
---
Service notes gathered while watching the dashboards:

- checkout-service: latency spikes at 02:00 are the nightly batch import, not
  an incident.
- search-api: the 99th-percentile panel is the reliable one; the mean panel
  hides tail problems.
- billing-worker: a queue depth above 5000 for more than 10 minutes is worth
  an issue even when the dashboard still says green.
