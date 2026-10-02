#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Apply the runtime settings of the Loongson-3 CPUFreq driver: load the module,
# select the governor and switch boost on.
#
# This is what loongson3-cpufreq.service runs at boot.  It exists as a separate
# script because both settings live on objects that are recreated whenever the
# module is (re)loaded:
#
#   * `scaling_governor` is a property of `struct cpufreq_policy`, which only
#     exists while the driver is registered -- a fresh policy always starts on
#     the kernel default governor (CONFIG_CPU_FREQ_DEFAULT_GOV_PERFORMANCE);
#   * `cpufreq_driver.boost_enabled` (the global `/sys/.../cpufreq/boost`) is
#     zero-initialised every time the driver registers.
#
# Settings come from the environment, normally /etc/default/loongson3-cpufreq.

set -eu

GOVERNOR=${GOVERNOR:-schedutil}
BOOST=${BOOST:-1}
MODULE=${MODULE:-loongson3_cpufreq}
MODPROBE=${MODPROBE:-/sbin/modprobe}

# `modprobe` is a no-op when the module is already loaded; doing it here
# guarantees the cpufreq policies exist before anything below touches them.
"$MODPROBE" "$MODULE"

if [ ! -d /sys/devices/system/cpu/cpu0/cpufreq ]; then
	echo "loongson3-cpufreq: $MODULE did not register a cpufreq policy" >&2
	exit 1
fi

# --- governor -------------------------------------------------------------
avail=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_available_governors)
case " $avail " in
*" $GOVERNOR "*)
	for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
		echo "$GOVERNOR" >"$g"
	done
	echo "loongson3-cpufreq: governor -> $GOVERNOR"
	;;
*)
	echo "loongson3-cpufreq: governor '$GOVERNOR' unavailable (kernel offers:$avail)" >&2
	;;
esac

# --- boost ----------------------------------------------------------------
if [ "$BOOST" = "1" ]; then
	if [ -w /sys/devices/system/cpu/cpufreq/boost ]; then
		echo 1 >/sys/devices/system/cpu/cpufreq/boost
		echo "loongson3-cpufreq: boost -> $(cat /sys/devices/system/cpu/cpufreq/boost)"
	else
		echo "loongson3-cpufreq: no global boost attribute, skipped" >&2
	fi
else
	echo "loongson3-cpufreq: boost left disabled"
fi
