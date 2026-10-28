#!/bin/sh
# The per-test body is read as input, never executed as a writable file.
exec /bin/sh "$0.sh" "$@"
