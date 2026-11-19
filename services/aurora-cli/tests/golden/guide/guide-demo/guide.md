# Guide demo (guide-demo)

Written by Aurora for this agent, except the last section.

## Accounts

Choose an account with the request header `x-aurora-connection-id: <connection id>`. Without it the gateway uses your only account, or refuses when there are several.

- `c_sample`: Sample account (host api.guide.example.test)

## Your access

- `c_sample`: custom access: 2 allowed, 1 need approval, 0 blocked

`tools.md` lists every operation and your access to it.

## Recipes

Step-by-step procedures from the Guide demo plugin (third-party). Open the one that matches your task before you start.

- `book-free-room.md`: Find a free room and book it

## Files in this folder

- `tools.md`: every operation and your access to it
- `api.json`: the operations of this app, for `aurora-agent`. Use `aurora-agent search|describe|call <app_id> …` rather than reading it.
- `api.md`: REST conventions, pagination, errors
- `codes.tsv`: Building codes (third-party file, no header)

## Documentation

- API documentation: https://docs.guide.example.test/api

## Notes from the Guide demo plugin (third-party)

```text
Rooms are addressed by numeric id. Send JSON bodies.
```
