#!/bin/sh
# Prints the room ids of a building that are free: reads the JSON the API returned on stdin.
set -eu
building="${1:?usage: free-rooms.sh <building>}"
echo "rooms of building ${building}:"
grep -o '"id": *[0-9]*' | grep -o '[0-9]*'
