#!/bin/sh
# Keep only the fixture container alive after the real server actor exits cleanly.
set -eu
umask 022
[ "$#" = 1 ] && [ "$1" = server ]
: "${SKVOZ_IP_WORK_END:?original qualifier work cutoff required}"
case "$SKVOZ_IP_WORK_END" in ''|*[!0-9]*) exit 125;; esac
now() { read -r uptime unused < /proc/uptime; printf '%s\n' "${uptime%%.*}"; }
remaining=$((SKVOZ_IP_WORK_END - $(now)))
[ "$remaining" -gt 1 ] || exit 124
mkdir -p /run/topology
status_file=/run/topology/server-actor.exit
publish_status() {
  printf '%s\n' "$1" > "$status_file.tmp"
  mv -f "$status_file.tmp" "$status_file"
}
publish_status unknown
actor=
stop() {
  trap '' HUP INT TERM
  if [ -n "$actor" ]; then
    kill -TERM "$actor" 2>/dev/null || true
    stop_end=$(($(now) + 1))
    [ "$stop_end" -le "$SKVOZ_IP_WORK_END" ] || stop_end=$SKVOZ_IP_WORK_END
    while kill -0 "$actor" 2>/dev/null && [ "$(now)" -lt "$stop_end" ]; do sleep 0.1; done
    if ! kill -0 "$actor" 2>/dev/null; then
      if wait "$actor"; then code=0; else code=$?; fi
      publish_status "$code"
    fi
  fi
  # Interrupted work is NONPASS; Tini/container cleanup reaps an uncertain child.
  exit 143
}
trap stop HUP INT TERM
/usr/bin/timeout --preserve-status --signal=TERM --kill-after=1 "$((remaining - 1))" /fixture/bin/ip_topology server &
actor=$!
if wait "$actor"; then code=0; else code=$?; fi
actor=
publish_status "$code"
[ "$code" = 0 ] || exit "$code"
# Product shutdown/child reaps/owned-kernel assertions already passed in Rust.
# Retaining PID1 lets the Docker exec shutdown requester finish before cleanup.
trap 'exit 0' HUP INT TERM
printf 'Server actor clean exit=0; fixture supervisor held for cleanup\n'
while [ "$(now)" -lt "$SKVOZ_IP_WORK_END" ]; do sleep 0.2; done
exit 124
