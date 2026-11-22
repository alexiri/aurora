# Guide demo API

This file covers conventions: ids, pagination and errors.

- Room ids are integers.
- List endpoints take `page` (from 1) and answer with a `Link` header.
- Errors are JSON `{ "error": "..." }`.
