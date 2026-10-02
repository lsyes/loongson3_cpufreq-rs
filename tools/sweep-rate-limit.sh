#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Sweep schedutil's rate limit and report, for each value, how many frequency
# transitions a 3-second two-thread load on one physical core causes and where
# the core ends up.  Use it to pick `transition_delay_us` from data instead of
# guessing: the number is a trade-off between ramp-up latency (smaller is
# snappier) and SMC mailbox traffic (larger is cheaper).
#
#   sudo tools/sweep-rate-limit.sh [us ...]      default: 200 500 1000 2000 5000
#
# `power-profiles-daemon` must not be switching the governor underneath; the
# script switches to schedutil itself.

set -eu
SELF=$(readlink -f "$0")
cd "$(dirname "$SELF")/.."

[ "$(id -u)" -eq 0 ] || { echo "please run through sudo: sudo $0" >&2; exit 1; }

TRACE=/sys/kernel/tracing
[ -d "$TRACE" ] || TRACE=/sys/kernel/debug/tracing
[ -d "$TRACE" ] || { echo "no tracefs found" >&2; exit 1; }

RATE=/sys/devices/system/cpu/cpufreq/schedutil/rate_limit_us
[ -w "$RATE" ] || { echo "$RATE not writable" >&2; exit 1; }

for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
	echo schedutil >"$g" 2>/dev/null || true
done

sweep() {
	us=$1
	echo "$us" >"$RATE"

	echo 0 >"$TRACE/tracing_on"
	echo >"$TRACE/trace"
	echo 1 >"$TRACE/events/power/cpu_frequency/enable"
	echo 1 >"$TRACE/tracing_on"

	# Two threads on cpu0/cpu1: one physical core, both SMT siblings busy.
	taskset -c 0 sh -c 'while :; do :; done' &
	p1=$!
	taskset -c 1 sh -c 'while :; do :; done' &
	p2=$!
	sleep 3
	kill "$p1" "$p2" 2>/dev/null || true
	wait 2>/dev/null || true

	echo 0 >"$TRACE/tracing_on"
	echo 0 >"$TRACE/events/power/cpu_frequency/enable"

	printf 'rate_limit=%-7s transitions/3s=%-6s top: %s\n' \
		"${us}us" \
		"$(grep -c 'cpu_frequency:' "$TRACE/trace")" \
		"$(grep -o 'state=[0-9]*' "$TRACE/trace" | sort | uniq -c |
			sort -rn | head -4 | awk '{printf "%s x%d  ", $2, $1}')"
}

if [ $# -gt 0 ]; then
	for us in "$@"; do sweep "$us"; done
else
	for us in 200 500 1000 2000 5000; do sweep "$us"; done
fi

echo
echo "Pick the smallest value whose transition count you find acceptable:"
echo "  smaller = snappier ramp-up, more SMC mailbox round trips."
