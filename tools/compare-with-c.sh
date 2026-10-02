#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Differential test against the original C driver.
#
# Loads the Rust module and the C module in turn with insmod/rmmod (nothing is
# installed permanently), snapshots everything the cpufreq core exposes through
# sysfs for each of them, and diffs the two snapshots.
#
#   . tools/rfl-env.sh && make          # build the Rust module first
#   sudo tools/compare-with-c.sh
#
# The C module is built on the fly from c-reference/ against the running
# kernel's linux-headers package -- the same way it was built before the rewrite.

set -eu
SELF=$(readlink -f "$0")
cd "$(dirname "$SELF")/.."

NAME=loongson3_cpufreq
KVER=$(uname -r)
KDIR=/lib/modules/$KVER/build
RUST_KO=$PWD/loongson3_cpufreq.ko

[ "$(id -u)" -eq 0 ] || {
	echo "please run through sudo: sudo $0" >&2
	exit 1
}
[ -f "$RUST_KO" ] || {
	echo "the Rust module is not built yet; run:" >&2
	echo "    . tools/rfl-env.sh && make" >&2
	exit 1
}

INSMOD=$(command -v insmod 2>/dev/null || echo /sbin/insmod)
RMMOD=$(command -v rmmod 2>/dev/null || echo /sbin/rmmod)

# ------------------------------------------------------- build the C module
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
echo "==> building the C driver from c-reference/ against $KDIR"
cp c-reference/loongson3_cpufreq.c "$TMP/"
printf 'obj-m := loongson3_cpufreq.o\n' >"$TMP/Makefile"
make -s -C "$KDIR" M="$TMP" modules >/dev/null
C_KO=$TMP/loongson3_cpufreq.ko

# ------------------------------------------------------------- helpers
snapshot() {
	for p in /sys/devices/system/cpu/cpu*/cpufreq; do
		cpu=${p#/sys/devices/system/cpu/}
		cpu=${cpu%/cpufreq}
		for f in scaling_driver scaling_governor scaling_min_freq \
			scaling_max_freq scaling_cur_freq cpuinfo_min_freq \
			cpuinfo_max_freq cpuinfo_cur_freq \
			scaling_available_frequencies scaling_available_governors \
			related_cpus affected_cpus boost; do
			[ -r "$p/$f" ] && printf '%s/%s = %s\n' "$cpu" "$f" "$(cat "$p/$f")"
		done
	done
	[ -r /sys/devices/system/cpu/cpufreq/boost ] &&
		printf 'global/boost = %s\n' "$(cat /sys/devices/system/cpu/cpufreq/boost)"
	[ -r /sys/devices/system/cpu/cpufreq/policy0/boost ] &&
		printf 'policy0/boost = %s\n' "$(cat /sys/devices/system/cpu/cpufreq/policy0/boost)"
	return 0
}

# Pin the governor so that the two runs are comparable.
pin_governor() {
	for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
		echo performance >"$g" 2>/dev/null || true
	done
}

unload() {
	# Nothing to do if it is not loaded; `rmmod` fails with ENOENT then, and with
	# EBUSY if something still holds a reference.
	if "$RMMOD" "$NAME" 2>/dev/null; then
		sleep 1
	elif grep -q "^$NAME " /proc/modules 2>/dev/null; then
		echo "error: $NAME is loaded but cannot be removed" >&2
		exit 1
	fi
}

run_one() {
	# $1 = module, $2 = output file, $3 = label
	unload
	"$INSMOD" "$1"
	sleep 1
	pin_governor
	sleep 1
	snapshot >"$2"
	echo "--- dmesg ($3) ---" >>"$2"
	dmesg | grep -i "Loongson-3 CPU frequency" | tail -n 1 >>"$2"
	unload
}

# ------------------------------------------------------------- run them
echo "==> loading the Rust module"
run_one "$RUST_KO" "$TMP/rust.txt" rust
echo "==> loading the C module"
run_one "$C_KO" "$TMP/c.txt" c

echo "==> restoring the Rust module"
unload
"$INSMOD" "$RUST_KO"

echo
if diff -u "$TMP/c.txt" "$TMP/rust.txt"; then
	echo "PASS: the C driver and the Rust driver expose identical cpufreq state"
else
	echo "FAIL: the two snapshots differ (C on the left, Rust on the right)" >&2
	exit 1
fi
