# Guide demo: operations and your access

Written by Aurora from the app's catalogue and your grants. The gateway decides every request;
workspace and organization policy can be stricter than this list, never looser.

## Accounts
- A: `c_sample` Sample account (custom access)

## Read
- `list_rooms` GET /v1/rooms: List rooms. Rooms of a building. [allowed]
- `get_room` GET /v1/rooms/*: Get a room. One room with its bookings. [allowed]

## Write
- `book_room` POST /v1/rooms/*/bookings: Book a room. Reserve a room for a time slot. [approval]

## Requests that match no operation above
[blocked]
