// SPDX-License-Identifier: GPL-2.0-only
//! Throwaway diagnostic: dumps the raw replies of the Loongson-3 SMC mailbox.
//!
//! It speaks the same protocol as the driver but prints, for every command, the
//! full 32-bit reply word instead of the fields the driver keeps, so that the
//! values the firmware really sends (level count, boost level, per-level
//! frequency) can be read without the driver's `FREQ_MAX_LEVEL` clamp in the way.
//!
//!     make -C tools/smc-dump            # or: sh tools/smc-dump/build.sh
//!     sudo insmod tools/smc-dump/smc_dump.ko && dmesg | tail -40
//!     sudo rmmod smc_dump
//!
//! It takes no mailbox lock: it runs once at load time, so do not load it while
//! something else is hammering the SMC.

use core::arch::asm;

use kernel::{bindings, prelude::*};

module! {
    type: SmcDump,
    name: "smc_dump",
    authors: ["diagnostic"],
    description: "Loongson-3 SMC mailbox raw dump",
    license: "GPL",
}

const IOCSR_SMCMBX: u32 = 0x51c;
const IOCSR_MISC_FUNC: u32 = 0x420;
const SOFT_INT: u32 = 1 << 10;

const CMD_GET_VERSION: u32 = 0x1;
const CMD_GET_FREQ_LEVEL_NUM: u32 = 0x9;
const CMD_GET_FREQ_BOOST_LEVEL: u32 = 0x10;
const CMD_GET_FREQ_LEVEL_INFO: u32 = 0x11;
const CMD_GET_FREQ_INFO: u32 = 0x12;

#[inline]
unsafe fn iocsr_read32(reg: u32) -> u32 {
    let mut v: u32;
    // SAFETY: plain IOCSR register read.
    unsafe { asm!("iocsrrd.w {0}, {1}", out(reg) v, in(reg) reg, options(nostack, preserves_flags)) };
    v
}

#[inline]
unsafe fn iocsr_write32(val: u32, reg: u32) {
    // SAFETY: plain IOCSR register write.
    unsafe { asm!("iocsrwr.w {0}, {1}", in(reg) val, in(reg) reg, options(nostack, preserves_flags)) };
}

fn request(id: u32, info: u32, cmd: u32, val: u32) -> u32 {
    let word = (id & 0xf) | ((info & 0xf) << 4) | ((val & 0xffff) << 8) | ((cmd & 0x3f) << 24);

    // SAFETY: the mailbox registers are always accessible from supervisor mode.
    let last = unsafe { iocsr_read32(IOCSR_SMCMBX) };
    if last & (1 << 31) == 0 {
        return 0xdead_beef; // mailbox busy
    }

    // SAFETY: see above.
    unsafe {
        iocsr_write32(word, IOCSR_SMCMBX);
        let misc = iocsr_read32(IOCSR_MISC_FUNC);
        iocsr_write32(misc | SOFT_INT, IOCSR_MISC_FUNC);
    }

    for _ in 0..10_000 {
        // SAFETY: see above.
        let reply = unsafe { iocsr_read32(IOCSR_SMCMBX) };
        if reply & (1 << 31) != 0 {
            return reply;
        }
        // SAFETY: process context, sleeping is allowed.
        unsafe { bindings::usleep_range_state(8, 12, bindings::TASK_UNINTERRUPTIBLE) };
    }
    0xdead_beef // timed out
}

fn dump(tag: &str, id: u32, info: u32, cmd: u32, val: u32) {
    let r = request(id, info, cmd, val);
    pr_info!(
        "{} (id={} info={} cmd={:#04x} val={}) -> raw={:#010x} complete={} cmd={:#04x} val={}\n",
        tag,
        id,
        info,
        cmd,
        val,
        r,
        r >> 31,
        (r >> 24) & 0x3f,
        (r >> 8) & 0xffff
    );
}

/// The diagnostic module.
struct SmcDump;

impl kernel::Module for SmcDump {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("=== SMC raw dump ===\n");
        dump("version          ", 0, 0, CMD_GET_VERSION, 0);
        for cpu in 0..8 {
            dump("freq_level_num   ", cpu, 0, CMD_GET_FREQ_LEVEL_NUM, 0);
        }
        for cpu in 0..8 {
            dump("freq_boost_level ", cpu, 0, CMD_GET_FREQ_BOOST_LEVEL, 0);
        }
        for cpu in 0..8 {
            dump("freq_info(cur)   ", cpu, 0, CMD_GET_FREQ_INFO, 0);
        }
        // The firmware reports 224 levels but the driver only keeps 16.  Dump the
        // first two blocks (0..31) and the region around the boost threshold (208).
        for level in 0..32 {
            dump("freq_level_info  ", 0, 0, CMD_GET_FREQ_LEVEL_INFO, level);
        }
        for level in 200..224 {
            dump("freq_level_info  ", 0, 0, CMD_GET_FREQ_LEVEL_INFO, level);
        }
        for cpu in 0..8 {
            dump("freq_level_info c1", cpu, 0, CMD_GET_FREQ_LEVEL_INFO, 15);
        }
        pr_info!("=== end of SMC raw dump ===\n");
        Ok(Self)
    }
}
