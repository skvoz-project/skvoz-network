#!/bin/sh
# Lifecycle glue only; functional assertions live in the Rust actors.
set -eu
export LC_ALL=C SYSTEMD_PAGER=cat
if [ "${1-}" != --run ]; then
  exec timeout --signal=TERM --kill-after=100 240 sh "$0" --run
fi
: "${SKVOZ_IP_PROBE:?set prepared probe path}"
: "${SKVOZ_IP_IMAGE:?set prepared fixture image digest}"
: "${SKVOZ_NATIVE_ROOT:?set verified extracted bundle path}"
: "${SKVOZ_NATS_BINARY:?set exact NATS 2.15.1 executable}"
: "${SKVOZ_BLOCK_DEVICE:?set backing device}"
: "${SKVOZ_IP_EVIDENCE:?set owning evidence directory}"
for tool in timeout docker journalctl grep sed tr wc sort cmp stat; do command -v "$tool" >/dev/null; done
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
project=skvoz-ip-$(date +%s)-$$
mkdir -m 700 "$SKVOZ_IP_EVIDENCE"
work=$SKVOZ_IP_EVIDENCE
export DOCKER_CONFIG=$work/docker-config
export DOCKER_HOST=unix:///var/run/docker.sock
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH DOCKER_TLS
mkdir -m 700 "$DOCKER_CONFIG"
printf '%s\n' "$project" > "$work/project"
printf 'exit=unknown\ncleanup=unknown\nkernel=unknown\n' > "$work/status"
: > "$work/commands-started"
: > "$work/commands-reaped"
now() { read -r uptime unused < /proc/uptime; printf '%s\n' "${uptime%%.*}"; }
# Keep the current timeout and reap it; detect a dead watcher during work,
# including long docker wait/build commands. Cleanup still removes resources.
bounded() {
  limit=$1; shift
  remaining=$((end - $(now)))
  [ "$remaining" -gt 1 ] || return 124
  [ "$limit" -lt "$remaining" ] || limit=$((remaining - 1))
  timeout --signal=TERM --kill-after=1 "$limit" "$@" &
  command_pid=$!
  printf '%s\n' "$command_pid" >> "$work/commands-started"
  broken=no
  while kill -0 "$command_pid" 2>/dev/null; do
    if [ -n "${watcher-}" ] && { ! kill -0 "$watcher" 2>/dev/null || [ -e "$SKVOZ_IP_EVIDENCE/kernel-failed" ]; }; then
      : > "$SKVOZ_IP_EVIDENCE/kernel-failed"
      if [ "${phase-work}" = work ] && [ "$broken" = no ]; then
        broken=yes
        kill -TERM "$command_pid" 2>/dev/null || true
      fi
    fi
    sleep 0.2
  done
  if wait "$command_pid"; then status=0; else status=$?; fi
  printf '%s\n' "$command_pid" >> "$work/commands-reaped"
  [ "$broken" = no ] || return 125
  return "$status"
}
storage_error='I/O error|Buffer I/O|blk_update_request|critical (medium|target|hardware)|EXT4-fs (error|warning)|XFS.*(error|corrupt)|BTRFS.*(error|corrupt)|nvme.*(timeout|reset|error)|ata[0-9].*(failed|error|reset)|usb.*(disconnect|reset|error|fail)|uas.*(abort|reset|error)|device descriptor read|rejecting I/O|read-only file system|reset.*USB|INFO:[[:space:]]+task[[:space:]]+.+:[0-9]+[[:space:]]+blocked[[:space:]]+for[[:space:]]+more[[:space:]]+than[[:space:]]*[0-9]+[[:space:]]+seconds'
kernel_read() (
  file=$1
  [ "$(tr -d '-' < /proc/sys/kernel/random/boot_id)" = "$boot" ] || exit 125
  if [ -z "$anchor" ]; then
    set -- --lines=1
  else
    set -- --cursor="$anchor" --lines=+4097
  fi
  (ulimit -f 2048; bounded 2 journalctl -k --boot="$boot" --no-pager --all \
    --output=json --output-fields=__CURSOR,_BOOT_ID,_TRANSPORT,MESSAGE "$@") > "$file" 2> "$file.err" || exit 125
  [ ! -s "$file.err" ] && [ -s "$file" ] || exit 125
  [ "$(wc -l < "$file")" -le 4096 ] || exit 125
  first=$(sed -n '1p' "$file")
  cursor=$(printf '%s\n' "$first" | sed -n 's/.*"__CURSOR"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  printf '%s\n' "$cursor" | grep -Eq "^s=[0-9a-f]+;i=[0-9a-f]+;b=$boot;m=[0-9a-f]+;t=[0-9a-f]+;x=[0-9a-f]+$" || exit 125
  [ -z "$anchor" ] || [ "$cursor" = "$anchor" ] || exit 125
  # Every JSON record must carry text and the expected kernel/boot identity.
  for field in '"MESSAGE"[[:space:]]*:[[:space:]]*"' \
    '"_TRANSPORT"[[:space:]]*:[[:space:]]*"kernel"' \
    "\"_BOOT_ID\"[[:space:]]*:[[:space:]]*\"$boot\""; do
    if grep -Ev "$field" "$file" > "$file.invalid"; then exit 125; else status=$?; fi
    [ "$status" = 1 ] || exit 125
  done
  if [ -n "$anchor" ]; then
    sed '1d' "$file" > "$file.new"
    if grep -Ei "$storage_error" "$file.new" > "$file.errors"; then exit 125; else status=$?; fi
    [ "$status" = 1 ] || exit 125
  fi
)
guard_start() {
  boot=$(tr -d '-' < /proc/sys/kernel/random/boot_id)
  [ "${#boot}" = 32 ]
  anchor=
  watcher=
  kernel_read "$SKVOZ_IP_EVIDENCE/kernel-anchor.json"
  anchor=$(sed -n 's/.*"__CURSOR"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$SKVOZ_IP_EVIDENCE/kernel-anchor.json")
  printf '%s\n' "$boot" "$anchor" > "$SKVOZ_IP_EVIDENCE/kernel-origin"
  (
    trap 'exit 125' HUP INT TERM
    end=$hard_end
    while [ ! -e "$SKVOZ_IP_EVIDENCE/kernel-stop" ]; do
      if ! kernel_read "$SKVOZ_IP_EVIDENCE/kernel-monitor.json"; then
        if [ ! -e "$SKVOZ_IP_EVIDENCE/kernel-failed" ]; then
          : > "$SKVOZ_IP_EVIDENCE/kernel-failed"
          kill -TERM "$$"
        fi
      fi
      [ ! -e "$SKVOZ_IP_EVIDENCE/kernel-stop" ] || break
      sleep 1
    done
    : > "$SKVOZ_IP_EVIDENCE/kernel-stopped"
  ) &
  watcher=$!
  kill -0 "$watcher"
}
guard_finish() {
  monitor_ok=no
  emergency=no
  if [ -n "${watcher-}" ] && kill -0 "$watcher" 2>/dev/null; then
    # Let the current read finish and reap its timeout before acknowledging stop.
    : > "$SKVOZ_IP_EVIDENCE/kernel-stop"
    stop_end=$(($(now) + 5))
    latest_stop=$((end - 3))
    [ "$stop_end" -le "$latest_stop" ] || stop_end=$latest_stop
    while kill -0 "$watcher" 2>/dev/null && [ "$(now)" -lt "$stop_end" ]; do sleep 0.2; done
    if kill -0 "$watcher" 2>/dev/null; then
      emergency=yes
      : > "$SKVOZ_IP_EVIDENCE/kernel-stop-emergency"
      kill -TERM "$watcher" 2>/dev/null || true
      term_end=$(($(now) + 1))
      [ "$term_end" -lt "$end" ] || term_end=$((end - 1))
      while kill -0 "$watcher" 2>/dev/null && [ "$(now)" -lt "$term_end" ]; do sleep 0.2; done
      if kill -0 "$watcher" 2>/dev/null; then kill -KILL "$watcher" 2>/dev/null || true; fi
      while kill -0 "$watcher" 2>/dev/null && [ "$(now)" -lt "$((end - 1))" ]; do sleep 0.2; done
    fi
    # Never block wait past the original reserve. Unreaped emergency is UNKNOWN.
    if kill -0 "$watcher" 2>/dev/null; then
      kernel_status=unknown
      return 125
    fi
    if wait "$watcher" 2>/dev/null && [ "$emergency" = no ] && [ -e "$SKVOZ_IP_EVIDENCE/kernel-stopped" ]; then monitor_ok=yes; fi
  elif [ -n "${watcher-}" ]; then
    wait "$watcher" 2>/dev/null || true
  fi
  watcher=
  kernel_status=unknown
  # The watcher has drained its bounded command before this final read.
  read_ok=no
  if kernel_read "$SKVOZ_IP_EVIDENCE/kernel-final.json"; then read_ok=yes; fi
  commands_ok=no
  if sort "$work/commands-started" > "$work/commands-started.sorted" && sort "$work/commands-reaped" > "$work/commands-reaped.sorted" && cmp "$work/commands-started.sorted" "$work/commands-reaped.sorted"; then commands_ok=yes; fi
  if [ "$read_ok" = yes ] && [ "$monitor_ok" = yes ] && [ "$commands_ok" = yes ] && [ ! -e "$SKVOZ_IP_EVIDENCE/kernel-failed" ]; then
    kernel_status=clean
  else return 125; fi
}

started=$(now)
end=$((started + 240))
cleanup_end=$((started + 320))
hard_end=$((started + 340))
export SKVOZ_IP_WORK_END=$end
scope=${SKVOZ_IP_SCOPE-full}
case "$scope" in full|server_shutdown) ;; *) echo 'Expected full or server_shutdown qualification scope' >&2; exit 2;; esac
printf '%s\n' "$scope" > "$work/scope"
printf 'work_end=%s\ncleanup_end=%s\nhard_end=%s\n' "$end" "$cleanup_end" "$hard_end" > "$work/bounds"
watcher=
anchor=
phase=work
project_owned=no
compose() {
  limit=$1; shift
  if [ -n "${SKVOZ_COMPOSE-}" ]; then
    bounded "$limit" "$SKVOZ_COMPOSE" -p "$project" -f "$root/qualification/ip/compose.yaml" "$@"
  else
    bounded "$limit" docker compose -p "$project" -f "$root/qualification/ip/compose.yaml" "$@"
  fi
}
# Each actor holds before its envelope or functional setup. Host metadata proof
# comes from the same owning probe, without any private adapter dependency.
start_role() {
  role=$1
  compose 230 up -d --no-deps "$role"
  id=$(compose 10 ps -aq "$role")
  printf '%s\n' "$id" | grep -Eq '^[0-9a-f]{64}$'
  printf '%s\n' "$id" > "$work/$role.id"
  bounded 10 docker inspect "$id" > "$work/$role.inspect.json"
  bounded 10 "$SKVOZ_IP_PROBE" --host-proof "$role" "$project" "$work/$role.inspect.json" "$work/$role.host.json"
  while :; do
    if bounded 10 docker exec "$id" test -f /run/ip-start-ready; then break; else result=$?; fi
    [ "$result" = 1 ] || return "$result"
    sleep 0.2
  done
  bounded 10 docker inspect "$id" > "$work/$role.release-inspect.json"
  bounded 10 "$SKVOZ_IP_PROBE" --host-recheck "$role" "$project" "$work/$role.release-inspect.json" "$work/$role.host.json"
  bounded 10 docker exec "$id" touch /run/ip-start-release
}
ready_role() {
  ready_id=$1
  while :; do
    health=$(bounded 10 docker inspect --format '{{.State.Status}} {{.State.Health.Status}}' "$ready_id")
    case "$health" in
      'running healthy') return 0;;
      'running starting') sleep 0.2;;
      *) return 125;;
    esac
  done
}
host_absent() {
  for gone_role in prepare endpoint server client; do
    [ -e "$work/$gone_role.id" ] || continue
    # A failed/missing proof cannot be silently promoted to cleanup PASS.
    [ -s "$work/$gone_role.host.json" ] || return 125
    bounded 10 "$SKVOZ_IP_PROBE" --host-cleanup "$work/$gone_role.host.json" > "$work/$gone_role.cleanup.json" || return 125
  done
}
# Exact owned project only. Successful fresh queries distinguish absence from
# a slow/unreadable daemon and prove no surviving resource users.
project_absent() {
  label=$1
  for kind in container network volume; do
    case "$kind" in container) set -- ps -aq;; network) set -- network ls -q;; volume) set -- volume ls -q;; esac
    file="$work/$label-$kind"
    bounded 10 docker "$@" --filter "label=com.docker.compose.project=$project" > "$file" 2> "$file.err" || return 125
    [ ! -s "$file" ] && [ ! -s "$file.err" ] || return 125
  done
}
cleanup() {
  code=$?
  phase=cleanup
  trap - EXIT
  trap '' HUP INT TERM
  end=$(($(now) + 80))
  [ "$end" -le "$cleanup_end" ] || end=$cleanup_end
  cleanup_status=unknown
  if [ "$project_owned" = yes ]; then
    compose 2 logs --no-color > "$work/actors.log" 2>&1 || code=125
    if compose 45 down --timeout 2 --volumes --remove-orphans > "$work/down.log" 2>&1; then
      if project_absent remaining && host_absent; then cleanup_status=absent; else code=125; fi
    else code=125; fi
  elif project_absent remaining; then cleanup_status=absent; else code=125; fi
  case "$code" in 124|125|137|143) cleanup_status=unknown;; esac
  # Cooperative drain, final kernel read and PID comparison share the original
  #20s result reserve; neither slow cleanup nor errors renew hard_end.
  end=$(($(now) + 20))
  [ "$end" -le "$hard_end" ] || end=$hard_end
  guard_finish || code=125
  printf 'exit=%s\ncleanup=%s\nkernel=%s\n' "$code" "$cleanup_status" "${kernel_status-unknown}" > "$work/status"
  exit "$code"
}
trap cleanup EXIT
trap 'exit 124' HUP INT TERM
guard_start
set -- $(bounded 10 stat -c '%t %T' "$SKVOZ_BLOCK_DEVICE")
[ "$#" = 2 ]
printf '%s\n' "$1" "$2" | grep -Eq '^[0-9a-f]+$'
[ -b "$SKVOZ_BLOCK_DEVICE" ]
SKVOZ_BLOCK_DEVICE_ID=$(printf '%d:%d' "0x$1" "0x$2")
export SKVOZ_BLOCK_DEVICE_ID
compose 230 config --quiet
project_absent before
project_owned=yes
start_role prepare
read -r prepare_id < "$work/prepare.id"
result=$(bounded 230 docker wait "$prepare_id")
[ "$result" = 0 ]
compose 10 rm -f prepare
bounded 10 "$SKVOZ_IP_PROBE" --host-cleanup "$work/prepare.host.json" > "$work/prepare.stage-cleanup.json"
# No prepare process overlaps the three live actor budgets.
start_role endpoint
read -r endpoint_id < "$work/endpoint.id"
ready_role "$endpoint_id"
start_role server
read -r server_id < "$work/server.id"
ready_role "$server_id"
if [ "$scope" = full ]; then
  start_role client
  read -r client < "$work/client.id"
  if bounded 230 docker wait "$client" > "$work/client-wait.stdout" 2> "$work/client-wait.stderr"; then client_wait_exit=0; else client_wait_exit=$?; fi
  printf '%s\n' "$client_wait_exit" > "$work/client-wait.exit"
  [ "$client_wait_exit" = 0 ]
  read -r result < "$work/client-wait.stdout"
  [ "$result" = 0 ]
else
  printf 'not_run:server_shutdown\n' > "$work/client-wait.scope"
fi
if compose 230 exec -T server /fixture/bin/ip_topology --shutdown > "$work/server-shutdown.stdout" 2> "$work/server-shutdown.stderr"; then server_exec_exit=0; else server_exec_exit=$?; fi
printf '%s\n' "$server_exec_exit" > "$work/server-shutdown.exit"
[ "$server_exec_exit" = 0 ]
read -r server_id < "$work/server.id"
while :; do
  if bounded 10 docker exec "$server_id" cat /run/topology/server-actor.exit > "$work/server-actor.stdout" 2> "$work/server-actor.stderr"; then actor_query_exit=0; else actor_query_exit=$?; fi
  printf '%s\n' "$actor_query_exit" > "$work/server-actor-query.exit"
  [ "$actor_query_exit" = 0 ]
  read -r actor_exit < "$work/server-actor.stdout"
  case "$actor_exit" in
    0) break;;
    unknown) sleep 0.2;;
    *) exit 125;;
  esac
done
printf 'Server shutdown requester and actor exit=0; Rust owned cleanup acknowledged\n' > "$work/server-closure"
