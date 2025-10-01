#!/bin/bash
# Admin utility to manage the feed generator

SOCKET_PATH="${ADMIN_SOCKET:-/var/run/noreposts-feed.sock}"

if [ ! -S "$SOCKET_PATH" ]; then
    echo "Error: Admin socket not found at $SOCKET_PATH"
    echo "Make sure the feed generator is running."
    exit 1
fi

# If command provided, execute it and exit
if [ $# -gt 0 ]; then
    echo "$@" | nc -U "$SOCKET_PATH"
else
    # Interactive mode
    nc -U "$SOCKET_PATH"
fi
