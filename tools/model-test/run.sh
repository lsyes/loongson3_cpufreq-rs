#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Differential test for the SMC mailbox state machine.
#
# model.c is a verbatim transcription of `do_service_request()` from the C
# driver; model.rs is a verbatim transcription of `smc::request()` from the Rust
# driver (message encoding included).  Both are driven by the same deterministic
# mock mailbox and their traces are compared byte for byte, which pins down:
#
#   * the on-the-wire message encoding (the C bitfield vs the explicit shifts),
#   * the order of the mailbox and MISC_FUNC writes,
#   * the number of mailbox polls and of usleep_range() calls,
#   * the result for every reply/status/timeout/busy scenario.
#
# Usage: tools/model-test/run.sh
#        RUSTC=... CC=... tools/model-test/run.sh

set -eu

cd "$(dirname "$0")"

RUSTC=${RUSTC:-rustc}
CC=${CC:-gcc}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

"$CC" -O2 -o "$TMP/model_c" model.c
"$RUSTC" -O -o "$TMP/model_rs" model.rs

"$TMP/model_c" >"$TMP/c.txt"
"$TMP/model_rs" >"$TMP/rs.txt"

if diff -u "$TMP/c.txt" "$TMP/rs.txt"; then
	echo "PASS: the C and the Rust state machine produce identical traces"
else
	echo "FAIL: the traces differ (C on the left, Rust on the right)" >&2
	exit 1
fi
