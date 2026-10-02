#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Build (as the calling user) and install (as root) the Rust module, then load
# it and print the result.
#
#   tools/install.sh            build + install + modprobe
#   tools/install.sh --remove   modprobe -r + delete the installed module
#
# It re-executes itself through sudo for the privileged half only, so the build
# artefacts stay owned by the caller, and it never relies on environment
# variables surviving sudo (sudo resets them by default).

set -eu
SELF=$(readlink -f "$0")
cd "$(dirname "$SELF")/.."

MOD=loongson3_cpufreq.ko
NAME=loongson3_cpufreq
KVER=$(uname -r)
DEST=/lib/modules/$KVER/updates/dkms

usage() {
	echo "usage: $0 [--remove] [--help]" >&2
	exit 1
}

REMOVE=0
while [ $# -gt 0 ]; do
	case $1 in
	--remove) REMOVE=1 ;;
	-h | --help) usage ;;
	*) usage ;;
	esac
	shift
done

# ---------------------------------------------------------------- user half
if [ "$(id -u)" -ne 0 ]; then
	if [ "$REMOVE" -eq 0 ]; then
		. tools/rfl-env.sh
		make
	fi
	echo
	echo "==> installing needs root, re-running through sudo"
	exec sudo "$SELF" "$@"
fi

# ---------------------------------------------------------------- root half
# /sbin is often missing from a non-root PATH; sudo's secure_path has it, but do
# not rely on that.
MODPROBE=$(command -v modprobe 2>/dev/null || echo /sbin/modprobe)
DEPMOD=$(command -v depmod 2>/dev/null || echo /sbin/depmod)

if [ "$REMOVE" -eq 1 ]; then
	"$MODPROBE" -r "$NAME" 2>/dev/null || true
	rm -f "$DEST/$MOD"
	"$DEPMOD" -a "$KVER"
	echo "removed $DEST/$MOD"
	exit 0
fi

[ -f "$MOD" ] || {
	echo "error: $MOD was not built; run 'make' first" >&2
	exit 1
}

if [ -f "$DEST/$MOD" ] && cmp -s "$MOD" "$DEST/$MOD"; then
	echo "==> $DEST/$MOD is already up to date"
else
	install -D -m 0644 "$MOD" "$DEST/$MOD"
	echo "==> installed $DEST/$MOD"
fi
"$DEPMOD" -a "$KVER"

# Reload: the module is very likely already loaded from an earlier install, and
# `modprobe` on an already-loaded module is a no-op, which would silently keep the
# previous build running.
"$MODPROBE" -r "$NAME" 2>/dev/null || true
sleep 1
"$MODPROBE" "$NAME"
echo "==> modprobe $NAME done"

# Make sure the kernel is really running the build we just installed: a stale
# module in /lib/modules (or an already-loaded one) is otherwise very easy to
# mistake for a change that "did not take effect".
want=$(/sbin/modinfo -F srcversion "$MOD" 2>/dev/null || true)
got=$(cat "/sys/module/$NAME/srcversion" 2>/dev/null || true)
if [ -n "$want" ] && [ "$want" != "$got" ]; then
	echo "*** WARNING: the loaded module (srcversion $got) is not the build" >&2
	echo "***          just installed (srcversion $want); the results below" >&2
	echo "***          do not reflect the current source." >&2
fi

# Module parameters, if the build has any: this is where a forgotten
# /etc/modprobe.d option shows up.
for p in "/sys/module/$NAME/parameters"/*; do
	[ -e "$p" ] || continue
	printf '%-32s %s\n' "param ${p##*/}" "$(cat "$p")"
done

echo
echo "==> result"
for f in scaling_driver scaling_cur_freq cpuinfo_cur_freq boost; do
	printf '%-32s %s\n' "cpu0/cpufreq/$f" \
		"$(cat "/sys/devices/system/cpu/cpu0/cpufreq/$f" 2>/dev/null || echo '(n/a)')"
done

avail=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_available_frequencies 2>/dev/null || true)
if [ -n "$avail" ]; then
	printf '%-32s %s levels, first=%s last=%s\n' "scaling_available_frequencies" \
		"$(echo "$avail" | wc -w)" \
		"$(echo "$avail" | awk '{print $1}')" \
		"$(echo "$avail" | awk '{print $NF}')"
fi
echo
dmesg | grep -i "Loongson-3 CPU frequency" | tail -n 3 || true
lsmod | grep "$NAME" || echo "(module not listed in lsmod)"
