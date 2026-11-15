# P5-CHAT-1: Quote-reply and files-only messages — API — **medium, phase 5, Rust**

## Goal
Back end for D-228: a message can reply to an earlier message of the same conversation (C-725), and a
message can be files only (C-726). The agent receives the quoted context, so a reply means something to it.

## Contracts (normative: `docs/API_CONTRACTS.md` C-725, C-726 — read both whole)
- `POST /v1/conversations/{id}/messages` accepts `reply_to: { seq }`; validation → 422
  `reply_target_invalid` with `details.reason` `not_found` | `not_replyable` | `redacted`.
- `turns.reply_to_seq bigint null`; `turn.queued` payload `reply_to?: { seq, author_label, author_kind, at,
  excerpt }` composed at insert (`sanitize::block`, 280 chars on a word boundary).
- Redaction cascade: redacting the target rewrites every referencing payload's `reply_to` to
  `{ seq, redacted: true }` in the same transaction; retention purges likewise.
- Delivery: the quote block prepended to `TurnInput.text` (exact format in C-725) for a turn, a steered
  follow-up and a promoted follow-up; stored `text` untouched.
- C-726: empty `text` allowed with attachments; delivered as `(sent N file(s))`; door copies name the files.

## Plan
1. Migration `crates/aurora-db/migrations/<next>_turn_reply_to.sql`: `turns.reply_to_seq bigint null` and a
   partial index `(conversation_id, reply_to_seq) where reply_to_seq is not null` (the redaction cascade
   finds referencing turns with it). Forward-only, no backfill. Take the next free migration number from
   `docs/PROGRESS.md` (lane allocations note) and record it there.
2. `services/aurora-api/src/domains/conversations/service.rs` (message create, ~line 329): parse and validate
   `reply_to` (look the event up by `(conversation_id, seq)`; type check; redacted check), compose the payload
   field, insert both turn and follow-up rows with it; C-726 text rule.
3. Redaction (`…/events/{seq}/redact`, `…/turns/{id}/redact`) and the retention purge: the cascade, in the
   same transaction as the redaction itself.
4. Dispatch (`turn.deliver` construction) and the steer path (`turn.message`) and promotion: compose the
   delivered text per C-725/C-726. One pure function `compose_delivered_text(text, reply, n_files)` with unit
   tests; every path calls it.
5. Channels: door copies and notifications that quote a person's text use file names when it is empty
   (grep the channels domain for the turn text; list the places in the commit).
6. Spec text that matches the contracts: `05 §Conversations and turns` (body fields, 422 code), `10 §Turns`
   and `§Transcript events` (`turn.queued` payload, delivery rule, redaction cascade), `03 §turns` (column),
   `16 §W5` only if `TurnInput` docs mention the text's provenance. OpenAPI and `web/src/lib/api/schema.d.ts`
   regenerated (`scripts/extract-schema.py` as the repo does).

## Owns
`crates/aurora-db/migrations/<next>_turn_reply_to.sql`, `crates/aurora-db/src/repo/{turns,transcript}.rs`
(as needed), `services/aurora-api/src/domains/conversations/**`, the dispatch/steer code that builds
`TurnInput`, the redaction and retention code paths, `web/openapi.json`, `web/src/lib/api/schema.d.ts`,
tests below, docs `05`, `10`, `03`. Not parallel with another lane on `domains/conversations` or a migration
in the same number range.

## Acceptance criteria
- [ ] IT: reply to an agent `text`, a `turn.queued`, an `attachment` → 201/200 and the payload; another
      conversation's seq → 422 `not_found`; a `tool.started` seq → `not_replyable`; a redacted target →
      `redacted`; reply on a follow-up row (active turn) works; idempotent replay returns the same row.
- [ ] IT: redact the target → referencing payloads become `{ seq, redacted: true }` in the same transaction
      (a failure rolls both back); export carries the field.
- [ ] Unit: `compose_delivered_text` — plain, reply, multi-line excerpt quoted line by line, redacted target,
      files-only, reply + files-only.
- [ ] IT (fake supervisor / delivery capture): the delivered `TurnInput.text` for a turn, a steered follow-up
      and a promoted follow-up carries the quote; the stored `text` does not.
- [ ] IT: files-only message accepted; empty text without files still 400; agent receives `(sent 2 files)`.
- [ ] `cargo fmt --check`, clippy, targeted IT files; migration applies on a scratch DB in sequence.

## Suggested agent/model and reason
Sonnet (`rust-developer`): one domain, one migration, clear contracts.
