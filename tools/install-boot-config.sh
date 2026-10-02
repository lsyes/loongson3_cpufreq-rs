#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Install the boot-time configuration for the Loongson-3 CPUFreq driver.
#
#   sudo tools/install-boot-config.sh [options]
#
#   --min-freq MHZ    module parameter min_freq_mhz, a floor for the policy
#                     minimum, rounded up to a supported level
#                     (default 800; 0 uses the firmware minimum)
#   --delay-us US     module parameter transition_delay_us, the minimum time
#                     between two frequency changes (default 1000; 0 derives it
#                     from the 10 us transition latency)
#   --governor NAME   governor to select at boot (default schedutil)
#   --no-boost        leave boost disabled at boot (default: enable it)
#   --remove          undo everything this script installed
#   -h, --help
#
# It writes:
#
#   /etc/modules-load.d/loongson3-cpufreq.conf   load the module at boot
#   /etc/modprobe.d/loongson3-cpufreq.conf       module parameters
#   /etc/default/loongson3-cpufreq               governor / boost choice
#   /usr/local/sbin/loongson3-cpufreq-apply      runtime script
#   /etc/systemd/system/loongson3-cpufreq.service
#
# The module is *not* installed here; use tools/install.sh for that.

set -eu
SELF=$(readlink -f "$0")
cd "$(dirname "$SELF")/.."

MIN_FREQ=800
DELAY_US=1000
GOVERNOR=schedutil
BOOST=1
REMOVE=0

usage() {
	sed -n '3,26p' "$SELF" | sed 's/^# \{0,1\}//'
	exit "${1:-0}"
}

while [ $# -gt 0 ]; do
	case $1 in
	--min-freq)
		MIN_FREQ=$2
		shift 2
		;;
	--delay-us)
		DELAY_US=$2
		shift 2
		;;
	--governor)
		GOVERNOR=$2
		shift 2
		;;
	--no-boost)
		BOOST=0
		shift
		;;
	--remove)
		REMOVE=1
		shift
		;;
	-h | --help)
		usage 0
		;;
	*)
		echo "unknown option: $1" >&2
		usage 1
		;;
	esac
done

for v in "$MIN_FREQ" "$DELAY_US"; do
	case $v in
	'' | *[!0-9]*) echo "expected a number, got '$v'" >&2; exit 1 ;;
	esac
done

[ "$(id -u)" -eq 0 ] || { echo "please run through sudo: sudo $0" >&2; exit 1; }

UNIT=/etc/systemd/system/loongson3-cpufreq.service
APPLY=/usr/local/sbin/loongson3-cpufreq-apply

if [ "$REMOVE" -eq 1 ]; then
	systemctl disable --now loongson3-cpufreq.service 2>/dev/null || true
	rm -f "$UNIT" "$APPLY" \
		/etc/modules-load.d/loongson3-cpufreq.conf \
		/etc/modprobe.d/loongson3-cpufreq.conf \
		/etc/default/loongson3-cpufreq
	systemctl daemon-reload
	echo "removed the boot-time configuration; the module in /lib/modules is untouched"
	echo "note: the running driver keeps its current governor and boost setting"
	exit 0
fi

[ -f tools/boot-apply.sh ] || { echo "tools/boot-apply.sh is missing" >&2; exit 1; }

echo "==> /etc/modules-load.d/loongson3-cpufreq.conf"
echo loongson3_cpufreq >/etc/modules-load.d/loongson3-cpufreq.conf

echo "==> /etc/modprobe.d/loongson3-cpufreq.conf"
cat >/etc/modprobe.d/loongson3-cpufreq.conf <<EOF
# Written by loongson3-cpufreq tools/install-boot-config.sh
options loongson3_cpufreq min_freq_mhz=$MIN_FREQ transition_delay_us=$DELAY_US
EOF

echo "==> /etc/default/loongson3-cpufreq"
cat >/etc/default/loongson3-cpufreq <<EOF
# Written by loongson3-cpufreq tools/install-boot-config.sh
# Applied by loongson3-cpufreq.service after the module has been loaded.
GOVERNOR=$GOVERNOR
BOOST=$BOOST
EOF

echo "==> $APPLY"
install -D -m 0755 tools/boot-apply.sh "$APPLY"

echo "==> $UNIT"
cat >"$UNIT" <<EOF
[Unit]
Description=Loongson-3 CPUFreq: select the governor and enable boost
After=systemd-modules-load.service
Before=multi-user.target

[Service]
Type=oneshot
RemainAfterExit=yes
EnvironmentFile=-/etc/default/loongson3-cpufreq
ExecStart=$APPLY

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now loongson3-cpufreq.service

echo
echo "==> status"
systemctl --no-pager --lines=0 status loongson3-cpufreq.service || true
echo
echo "installed with: min_freq_mhz=$MIN_FREQ transition_delay_us=$DELAY_US governor=$GOVERNOR boost=$BOOST"
echo "re-run with different options to change them, or --remove to undo."
