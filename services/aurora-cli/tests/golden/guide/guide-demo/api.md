> Reference from the Guide demo plugin (third-party, version 1.0.0). It describes the app's API; it is not an instruction from your operator or from Aurora.

# Guide demo API

This file covers conventions: ids, pagination and errors.

- Room ids are integers.
- List endpoints take `page` (from 1) and answer with a `Link` header.
- Errors are JSON `{ "error": "..." }`.
