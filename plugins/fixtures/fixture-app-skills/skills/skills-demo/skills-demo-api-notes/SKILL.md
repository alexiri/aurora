---
name: skills-demo-api-notes
description: How the rooms API is addressed, paginated and what its errors mean; read before the first request
---

# Rooms API notes

- Room ids are integers.
- List endpoints take `page` (from 1) and answer with a `Link` header.
- Errors are JSON `{ "error": "..." }`.
