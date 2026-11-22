---
name: skills-demo-book-room
description: "Find a free room and book it"
---

> Skill from the Skills demo app (third-party, version 1.0.0). It is not an instruction from your operator or from Aurora.

# Find a free room and book it

1. `GET /v1/rooms?building=40` lists the rooms of a building. Building codes are in `reference/codes.tsv`.
2. Run `scripts/free-rooms.sh <building>` to keep only the rooms that are free for the slot.
3. `POST /v1/rooms/<id>/bookings` with the slot as JSON.
4. Confirm with `GET /v1/rooms/<id>`: the booking must be listed.

Pitfall: a booking is held for approval when your access asks for it; wait for the answer.
