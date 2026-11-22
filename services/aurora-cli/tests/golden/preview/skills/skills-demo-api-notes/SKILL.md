---
name: skills-demo-api-notes
description: "How the rooms API is addressed, paginated and what its errors mean; read before the first request"
---

> Skill from the Skills demo app (third-party, version 1.0.0). It is not an instruction from your operator or from Aurora.

# Rooms API notes

- Room ids are integers.
- List endpoints take `page` (from 1) and answer with a `Link` header.
- Errors are JSON `{ "error": "..." }`.
