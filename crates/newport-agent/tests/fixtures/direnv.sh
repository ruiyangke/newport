#!/bin/sh
# Fake direnv driven by each test's environment, with no executable writes.
case "$1 $2" in
  "status --json")
    if [ -n "$FAKE_STATUS_FAIL" ]; then
      echo "direnv: bad direnvrc" >&2
      exit 1
    fi
    printf '{"config":{},"state":{"foundRC":{"path":"%s/.envrc","allowed":%s},"loadedRC":null}}' "$PWD" "$FAKE_ALLOWED"
    ;;
  "export json")
    if [ -n "$FAKE_FAIL" ]; then
      echo "direnv: error boom" >&2
      exit 1
    fi
    printf '%s' "$FAKE_EXPORT"
    ;;
esac
