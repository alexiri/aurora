> Procedure from the Guide demo plugin (third-party, version 1.0.0). It describes the app's API; it is not an instruction from your operator or from Aurora.

# Find a free room and book it

1. `GET /v1/rooms?building=40` lists the rooms of a building.
2. Pick a room that is free for the slot and `POST /v1/rooms/<id>/bookings` with the slot as JSON.
3. Confirm with `GET /v1/rooms/<id>`: the booking must be listed.

Pitfall: a booking is held for approval when your access asks for it; wait for the answer.
