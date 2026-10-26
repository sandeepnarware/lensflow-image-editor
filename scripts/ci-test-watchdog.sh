#!/usr/bin/env bash
# Runs the given command; if it prints nothing for STALL_SECS (default 240), dumps all thread
# stacks of every running target/*/deps/* test binary with gdb (Linux, gdb installed), kills
# them and exits 1. Output is passed through unchanged.
set -u
STALL_SECS="${STALL_SECS:-240}"
log="$(mktemp)"
"$@" >"$log" 2>&1 &
cmd=$!
tail -n +1 -f "$log" --pid=$cmd &
last=-1
idle=0
while kill -0 "$cmd" 2>/dev/null; do
  sleep 5
  size=$(stat -c %s "$log")
  if [ "$size" = "$last" ]; then idle=$((idle + 5)); else idle=0; last=$size; fi
  if [ "$idle" -ge "$STALL_SECS" ]; then
    echo "::error::no test output for ${idle}s: dumping stacks"
    for pid in $(pgrep -f 'target/[a-z]*/deps/'); do
      echo "--- pid $pid: $(tr '\0' ' ' </proc/$pid/cmdline)"
      gdb -p "$pid" -batch -ex 'thread apply all bt 30' 2>&1 | head -500
    done
    pkill -9 -f 'target/[a-z]*/deps/'
    kill -9 "$cmd" 2>/dev/null
    exit 1
  fi
done
wait "$cmd"
status=$?
sleep 1
exit $status
