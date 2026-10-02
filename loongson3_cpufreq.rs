// SPDX-License-Identifier: GPL-2.0-only
//! CPUFreq driver for the Loongson-3 processors.
//!
//! All revisions of the Loongson-3 processor support the `cpu_has_scalefreq` feature.
//!
//! This is a Rust rewrite of `drivers/cpufreq/loongson3_cpufreq.c`. The driver does not
//! program PLLs or dividers itself: frequency scaling, the frequency table and the
//! DVFS feature switches are all delegated to the SMC (System Management Controller)
//! firmware, which is reached through the mailbox register that LoongArch exposes in
//! IOCSR space.
//!
//! This is a rewrite, not a translation: the C driver is reproduced command for command,
//! but the defects that make it misbehave on real firmware are fixed.  Every deliberate
//! departure is called out in a `NOTE:` comment next to the code and summarised in
//! README.md; the three that matter on a Loongson-3A6000 are
//!
//!   * the firmware reports frequencies in 1/16 MHz, not MHz;
//!   * clamping the table to `FREQ_MAX_LEVEL = 16` keeps only the lowest of the 224
//!     levels the firmware offers, so the CPU could never be scaled above 375 MHz;
//!   * the default (suspend) level was computed as `boost_level - 1` without bounds
//!     checks, which is an out-of-bounds read as soon as the table is clamped.
//!
//! C header: [`include/linux/cpufreq.h`](srctree/include/linux/cpufreq.h)

use core::ptr;

use kernel::{
    bindings,
    clk::Hertz,
    cpu::CpuId,
    cpufreq,
    cpumask::Cpumask,
    device::Core,
    error::to_result,
    platform,
    prelude::*,
    str::CStr,
    sync::atomic::{
        Atomic,
        ordering::Relaxed,
    },
};

/// The SMC mailbox protocol.
///
/// Mirrors the definitions at the top of the C driver, which in turn mirror the
/// interface documented by the Loongson-3 firmware.
mod smc {
    use super::*;

    /// IOCSR address of the SMC mailbox (`LOONGARCH_IOCSR_SMCMBX`).
    const IOCSR_SMCMBX: u32 = 0x51c;

    /// IOCSR address of the miscellaneous function register (`LOONGARCH_IOCSR_MISC_FUNC`).
    const IOCSR_MISC_FUNC: u32 = 0x420;

    /// `IOCSR_MISC_FUNC_SOFT_INT`: raise the soft interrupt that tells the SMC to look
    /// at the mailbox.
    const MISC_FUNC_SOFT_INT: u32 = 1 << 10;

    /// How often the mailbox is polled before the request is given up on.
    const REQUEST_RETRIES: usize = 10_000;

    /// Sleep interval between two mailbox polls, in microseconds.
    const POLL_MIN_US: usize = 8;
    const POLL_MAX_US: usize = 12;

    /// `CMD_OK`: the SMC served the request without error.
    const CMD_OK: u32 = 0;

    /// `CMD_GET_VERSION`: get the version of the SMC interface.
    pub(super) const CMD_GET_VERSION: u32 = 0x1;

    /// `CMD_SET_FEATURE`: set the state of a feature.
    pub(super) const CMD_SET_FEATURE: u32 = 0x3;

    /// `CMD_GET_FREQ_LEVEL_NUM`: get the number of DVFS frequency levels.
    pub(super) const CMD_GET_FREQ_LEVEL_NUM: u32 = 0x9;

    /// `CMD_GET_FREQ_BOOST_LEVEL`: get the first boost frequency level.
    pub(super) const CMD_GET_FREQ_BOOST_LEVEL: u32 = 0x10;

    /// `CMD_GET_FREQ_LEVEL_INFO`: get the frequency of a DVFS level.
    pub(super) const CMD_GET_FREQ_LEVEL_INFO: u32 = 0x11;

    /// `CMD_GET_FREQ_INFO`: get a piece of frequency information of a CPU.
    pub(super) const CMD_GET_FREQ_INFO: u32 = 0x12;

    /// `CMD_SET_FREQ_INFO`: set a piece of frequency information of a CPU.
    pub(super) const CMD_SET_FREQ_INFO: u32 = 0x13;

    /// `FEATURE_DVFS`: the DVFS feature.
    pub(super) const FEATURE_DVFS: u32 = 2;

    /// `FEATURE_DVFS_ENABLE`: turn DVFS on.
    pub(super) const FEATURE_DVFS_ENABLE: u32 = 1 << 0;

    /// `FEATURE_DVFS_BOOST`: allow frequencies above the nominal maximum.
    pub(super) const FEATURE_DVFS_BOOST: u32 = 1 << 1;

    /// `FREQ_INFO_TYPE_FREQ`: the `val` of a request carries/returns a frequency.
    pub(super) const FREQ_INFO_TYPE_FREQ: u32 = 0;

    /// `FREQ_INFO_TYPE_LEVEL`: the `val` of a request carries/returns a level.
    pub(super) const FREQ_INFO_TYPE_LEVEL: u32 = 1;

    /// Reads a 32-bit IOCSR register of the CPU this runs on.
    ///
    /// This is the Rust spelling of the C `iocsr_read32()` macro. GCC implements the
    /// macro through a builtin; the instruction is emitted directly here, since the
    /// kernel's Rust abstractions do not cover IOCSR space.
    ///
    /// # Safety
    ///
    /// `reg` must be an IOCSR address that is readable from supervisor mode.
    #[inline]
    unsafe fn iocsr_read32(reg: u32) -> u32 {
        let mut val: u32;

        // SAFETY: `iocsrrd.w` is a plain register read: it does not touch memory or the
        // stack and does not clobber the condition flags. The value written to the
        // output register is always initialised by the instruction.
        //
        // `nomem` is deliberately *not* requested: two reads of the same mailbox must
        // not be coalesced by the optimiser, as the whole protocol is a poll on a
        // register that firmware updates behind our back.
        unsafe {
            core::arch::asm!(
                "iocsrrd.w {0}, {1}",
                out(reg) val,
                in(reg) reg,
                options(nostack, preserves_flags),
            )
        };

        val
    }

    /// Writes a 32-bit IOCSR register of the CPU this runs on.
    ///
    /// # Safety
    ///
    /// `reg` must be an IOCSR address that is writable from supervisor mode.
    #[inline]
    unsafe fn iocsr_write32(val: u32, reg: u32) {
        // SAFETY: `iocsrwr.w` is a plain register write: it does not touch memory or the
        // stack and does not clobber the condition flags.
        unsafe {
            core::arch::asm!(
                "iocsrwr.w {0}, {1}",
                in(reg) val,
                in(reg) reg,
                options(nostack, preserves_flags),
            )
        };
    }

    /// Sleeps for an interval of roughly `[min_us, max_us]` microseconds.
    ///
    /// The Rust abstractions only expose a busy-waiting `udelay()`; a mailbox round trip
    /// can take tens of microseconds, so the SMC is polled with a sleeping delay, just
    /// like the C driver does.
    fn usleep_range(min_us: usize, max_us: usize) {
        // SAFETY: `usleep_range_state()` may sleep, which every caller of
        // `service_request()` is allowed to do: it holds a mutex, and the cpufreq core
        // calls the driver callbacks from process context.
        unsafe {
            bindings::usleep_range_state(min_us, max_us, bindings::TASK_UNINTERRUPTIBLE)
        };
    }

    /// A 32-bit SMC mailbox message.
    ///
    /// Rust equivalent of the C `union smc_message`. The bitfield layout is spelled out
    /// explicitly rather than relying on a C bitfield, which makes the encoding
    /// independent of the compiler and of the endianness of the target:
    ///
    /// ```text
    ///   31    30   29 .. 24   23 .. 8   7 .. 4   3 .. 0
    /// +------+----+---------+--------+--------+-------+
    /// |compl.|ext.|   cmd   |  val   |  info  |  id   |
    /// +------+----+---------+--------+--------+-------+
    /// ```
    #[derive(Clone, Copy)]
    #[repr(transparent)]
    struct Message(u32);

    impl Message {
        /// Builds a request message with `complete` cleared, which asks the SMC to
        /// serve it.
        const fn request(id: u32, info: u32, cmd: u32, val: u32, extra: bool) -> Self {
            Self(
                (id & 0xf)
                    | ((info & 0xf) << 4)
                    | ((val & 0xffff) << 8)
                    | ((cmd & 0x3f) << 24)
                    | ((extra as u32) << 30),
            )
        }

        /// Wraps a raw register value.
        const fn from_raw(value: u32) -> Self {
            Self(value)
        }

        /// The raw register value.
        const fn into_raw(self) -> u32 {
            self.0
        }

        /// `msg.complete`: set by the firmware once it has served the request.
        const fn is_complete(self) -> bool {
            self.0 & (1 << 31) != 0
        }

        /// `msg.cmd`: the command return value, one of the `CMD_*` constants.
        const fn status(self) -> u32 {
            (self.0 >> 24) & 0x3f
        }

        /// `msg.val`: the payload of the reply.
        const fn value(self) -> u32 {
            (self.0 >> 8) & 0xffff
        }
    }

    kernel::sync::global_lock! {
        // SAFETY: The lock is initialised by `smc::init()` before the platform driver
        // can be probed, hence before the first request is issued.
        unsafe(uninit) static MAILBOX: Mutex<()> = ();
    }

    /// Whether [`init`] already initialised [`MAILBOX`].
    static MAILBOX_READY: Atomic<bool> = Atomic::new(false);

    /// Initialises the mailbox lock.
    ///
    /// The C driver initialises one mutex per physical package in its `probe()`
    /// callback. A `GlobalLock` may only be initialised once, while `probe()` runs
    /// again whenever the platform device is bound after being unbound, so the
    /// initialisation is latched.
    pub(super) fn init() {
        if MAILBOX_READY.cmpxchg(false, true, Relaxed).is_err() {
            return;
        }

        // SAFETY: The successful `compare_exchange()` above guarantees that this runs at
        // most once per module load, which is what `GlobalLock::init()` requires.
        unsafe { MAILBOX.init() };
    }

    /// Sends one request to the SMC and returns the payload of its reply.
    ///
    /// This is the Rust equivalent of the C `do_service_request()`.
    ///
    /// # Errors
    ///
    /// [`EPERM`] if the mailbox is still busy with a previous transaction, if the
    /// firmware did not answer within [`REQUEST_RETRIES`] polls, or if it answered with
    /// a status other than [`CMD_OK`]. The C driver reports every one of these as
    /// `-EPERM` as well.
    pub(super) fn request(id: u32, info: u32, cmd: u32, val: u32, extra: bool) -> Result<u32> {
        // NOTE: the C driver picks the mutex of the package the *calling* CPU belongs to,
        // even though `id` may name a core of a different package. Since the mailbox is
        // the one of the calling CPU's IOCSR space, a single lock that serialises all
        // mailbox users is a strict superset of what the C driver does, and it cannot
        // pick the wrong package.
        let _guard = MAILBOX.lock();

        // SAFETY: `IOCSR_SMCMBX` is the mailbox register; reading it has no side effects.
        let last = Message::from_raw(unsafe { iocsr_read32(IOCSR_SMCMBX) });
        if !last.is_complete() {
            return Err(EPERM);
        }

        let msg = Message::request(id, info, cmd, val, extra);

        // SAFETY: `IOCSR_SMCMBX` and `IOCSR_MISC_FUNC` are writable SMC registers.
        unsafe {
            iocsr_write32(msg.into_raw(), IOCSR_SMCMBX);

            let misc_func = iocsr_read32(IOCSR_MISC_FUNC);
            iocsr_write32(misc_func | MISC_FUNC_SOFT_INT, IOCSR_MISC_FUNC);
        }

        for _ in 0..REQUEST_RETRIES {
            // SAFETY: `IOCSR_SMCMBX` is the mailbox register.
            let reply = Message::from_raw(unsafe { iocsr_read32(IOCSR_SMCMBX) });

            if reply.is_complete() {
                return if reply.status() == CMD_OK {
                    Ok(reply.value())
                } else {
                    Err(EPERM)
                };
            }

            usleep_range(POLL_MIN_US, POLL_MAX_US);
        }

        Err(EPERM)
    }
}

/// Upper bound on the number of firmware levels the driver builds a table for.
///
/// NOTE: the C driver used `FREQ_MAX_LEVEL = 16`, which on real firmware keeps
/// only the *lowest* block of the table: a Loongson-3A6000 reports 224 levels
/// (375 MHz to 2600 MHz in 14 blocks of 16 sub-steps), so the C driver pinned
/// `policy->freq_table` to 375.0-375.9 MHz and the `performance` governor could
/// never ask for more than that.  All reported levels are used now; the constant
/// only guards against firmware that would otherwise make the driver allocate
/// without bound (227 levels x 12 bytes is well under a page).
const FREQ_MAX_LEVEL: usize = 1024;

/// Transition latency advertised to the cpufreq core, in nanoseconds.
const TRANSITION_LATENCY_NS: u32 = 10_000;

/// Converts a firmware frequency value into kHz.
///
/// NOTE: the firmware reports frequencies in units of 1/16 MHz, not MHz.  A
/// Loongson-3A6000 answers `40000` for its 2500 MHz nominal level and `41600`
/// for the 2600 MHz boost block, and the 375 MHz block starts at `6000`.  The C
/// driver treated the raw value as MHz (`ret * KILO`), which published every
/// frequency exactly 16 times too high -- a 375.9 MHz level showed up as
/// 6015000 kHz, hence the "6 GHz" in `scaling_available_frequencies`.
///
/// 1/16 MHz == 62.5 kHz, so `value * 125 / 2` keeps the half-kilohertz until the
/// final division.
fn freq_to_khz(value: u32) -> usize {
    (value as usize * 125) / 2
}

/// Returns the `cpuinfo_loongarch` entry of `cpu`.
///
/// `cpu_data` is declared as `extern struct cpuinfo_loongarch cpu_data[]`, so `bindgen`
/// can only describe it as a zero-length array. It really holds one entry per possible
/// CPU and is never freed.
fn cpu_info(cpu: CpuId) -> &'static bindings::cpuinfo_loongarch {
    let base = ptr::addr_of_mut!(bindings::cpu_data).cast::<bindings::cpuinfo_loongarch>();

    // SAFETY: `cpu` is a valid CPU number, so the index is in bounds of `cpu_data`, and
    // the array is a global that lives for as long as the kernel does.
    unsafe { &*base.add(cpu.as_u32() as usize) }
}

/// Returns the physical core number of `cpu` within its package.
///
/// This is what the firmware expects as the `id` of a DVFS request, mirroring the C
/// driver's `cpu_data[policy->cpu].core`.
fn core_of(cpu: CpuId) -> u32 {
    cpu_info(cpu).core as u32
}

/// Returns the CPUs that share the frequency domain of `cpu`.
///
/// `topology_sibling_cpumask()` is a macro that expands to `&cpu_sibling_map[cpu]`, so
/// the exported array is used directly; the kernel's Rust abstractions do not wrap
/// topology masks yet.
fn sibling_cpumask(cpu: CpuId) -> &'static Cpumask {
    let base = ptr::addr_of_mut!(bindings::cpu_sibling_map).cast::<bindings::cpumask>();

    // SAFETY: `cpu` is a valid CPU number, so the index is in bounds of
    // `cpu_sibling_map`, and the array is a global that lives for as long as the kernel
    // does.
    unsafe { Cpumask::from_raw(base.add(cpu.as_u32() as usize)) }
}

/// The per-policy state of the driver.
struct FreqData {
    /// The frequency table handed to the cpufreq core.
    ///
    /// The core dereferences `policy->freq_table` until the policy is torn down, so the
    /// table must be kept alive exactly as long as the policy owns this `FreqData`.
    table: cpufreq::TableBox,

    /// The level a policy is reset to when it goes away.
    default_level: usize,

    /// Number of usable entries in `table`, terminator excluded.
    levels: usize,
}

impl FreqData {
    /// Asks the firmware for the frequency table of `cpu`.
    ///
    /// NOTE: the C driver cached the table in a per-CPU pointer and reused it if the same
    /// CPU ever went through `->init()` twice, which only happens when a whole policy is
    /// torn down (CPU removal) and built again. The table is queried afresh here: the
    /// firmware is the single source of truth for it, and a cache that outlives the
    /// policy it was built for is exactly what made the C version leave dangling
    /// per-CPU pointers behind when the platform device went away.
    fn new(cpu: CpuId) -> Result<Self> {
        let max_level =
            smc::request(cpu.as_u32(), 0, smc::CMD_GET_FREQ_LEVEL_NUM, 0, false)?;
        let boost_level =
            smc::request(cpu.as_u32(), 0, smc::CMD_GET_FREQ_BOOST_LEVEL, 0, false)?;

        let levels = (max_level as usize).min(FREQ_MAX_LEVEL);

        // NOTE: the C driver built a table that only held the terminator when the
        // firmware reported no level at all, and then indexed it with `boost_level - 1`.
        // There is nothing sensible to do with such a firmware, so bail out instead.
        if levels == 0 {
            return Err(ENODEV);
        }

        let mut table = cpufreq::TableBuilder::new();

        // The table always describes the *hardware* range the firmware offers, whether or
        // not a floor is configured: `cpufreq_frequency_table_cpuinfo()` turns it into
        // `cpuinfo.min_freq`/`cpuinfo.max_freq`, and tools read those as the hardware
        // limits (`lscpu -e` prints them as MINMHZ/MAXMHZ).  A floor is a policy, not a
        // capability, so it goes into `policy->min` (`scaling_min_freq`) instead of
        // being baked into the table.
        for level in 0..levels {
            let freq_raw = smc::request(
                cpu.as_u32(),
                smc::FREQ_INFO_TYPE_FREQ,
                smc::CMD_GET_FREQ_LEVEL_INFO,
                level as u32,
                false,
            )?;
            let freq_khz = freq_to_khz(freq_raw);

            // Levels at or above `boost_level` may only be used when the user turns
            // boost on.
            let flags = if level as u32 >= boost_level {
                bindings::CPUFREQ_BOOST_FREQ
            } else {
                0
            };

            table.add(Hertz::from_khz(freq_khz), flags, 0)?;
        }

        // The default level is the fastest level that is still within specifications,
        // i.e. the last one below `boost_level`.
        //
        // NOTE: the C driver computed `boost_level - 1` unchecked, which underflows when
        // the firmware reports `boost_level == 0` and walks off the end of the table
        // when it reports `boost_level > levels`; both were read back as
        // `policy->suspend_freq`. The saturating subtraction and the clamp keep the
        // index inside the table.
        let default_level = (boost_level.saturating_sub(1) as usize).min(levels - 1);

        Ok(Self {
            table: table.to_table()?,
            default_level,
            levels,
        })
    }

    /// Returns the first level of the table that runs at or above `khz`.
    ///
    /// Used to turn the `min_freq_mhz` module parameter into a level that really
    /// exists, so that `scaling_min_freq` reports the effective floor instead of a
    /// value the core would silently round up later.
    fn round_up(&self, khz: usize) -> Result<usize> {
        for level in 0..self.levels {
            // SAFETY: `level` is below `self.levels`, so it is a valid table index.
            let index = unsafe { cpufreq::TableIndex::new(level) };

            if self.table.freq(index)?.as_khz() >= khz {
                return Ok(level);
            }
        }

        Ok(self.levels - 1)
    }

    /// The frequency the policy is reset to when it goes away.
    fn default_freq(&self) -> Result<Hertz> {
        // SAFETY: `default_level` is within `0..levels` by construction, i.e. it is a
        // valid index into `self.table`.
        let index = unsafe { cpufreq::TableIndex::new(self.default_level) };

        self.table.freq(index)
    }
}

/// Asks the firmware to switch the core that runs `policy` to frequency `level`.
fn set_freq_level(policy: &mut cpufreq::Policy, level: usize) -> Result {
    let core = core_of(policy.cpu());
    let level = u32::try_from(level)?;

    // The C driver maps any non-negative firmware status to success, which is what
    // `Ok(_)` means here.
    smc::request(
        core,
        smc::FREQ_INFO_TYPE_LEVEL,
        smc::CMD_SET_FREQ_INFO,
        level,
        false,
    )
    .map(|_| ())
}

/// Prints the driver banner.
///
/// The C driver calls `pr_info()`, which only prepends a module name when the
/// source file defines `pr_fmt()` — and `loongson3_cpufreq.c` never did, so its
/// banner reaches dmesg as a bare `cpufreq: Loongson-3 CPU frequency driver.`.
/// `pr_info!()` always prepends [`kernel::ModuleMetadata::NAME`] instead, so the
/// message is handed to `_printk()` directly to keep the output identical.
fn print_banner() {
    // `KERN_INFO` (0x01 '6') followed by the very same message the C driver
    // prints.  There are no conversion specifiers, so `_printk()` is called
    // without any variadic argument.
    const BANNER: &CStr = c"\x016cpufreq: Loongson-3 CPU frequency driver.\n";

    // SAFETY: `_printk()` is the kernel's printk entry point and is exported to
    // modules, `BANNER` is a NUL-terminated C string that lives forever, and it
    // contains no conversion specifiers, so passing no arguments is
    // well-defined.
    unsafe { bindings::_printk(BANNER.as_char_ptr()) };
}

/// The Loongson-3 CPUFreq driver.
struct Loongson3Cpufreq;

#[vtable]
impl cpufreq::Driver for Loongson3Cpufreq {
    const NAME: &'static CStr = c"loongson3";

    const FLAGS: u16 = cpufreq::flags::CONST_LOOPS;

    /// Maps to `cpufreq_driver.boost_enabled`, i.e. the *current* global boost state and
    /// not "this driver knows about boost".
    ///
    /// The C driver leaves the field at its zero-initialised value, so boost starts out
    /// disabled: `/sys/devices/system/cpu/cpufreq/boost` reads `0` until userspace turns
    /// it on, and only then does the per-policy `boost` attribute become writable.
    const BOOST_ENABLED: bool = false;

    type PData = KBox<FreqData>;

    fn init(policy: &mut cpufreq::Policy) -> Result<Self::PData> {
        let cpu = policy.cpu();

        // `set_freq_table()` below hands a pointer to the table over to the C code, so
        // refuse to run twice on the same policy.
        if policy.data::<Self::PData>().is_some() {
            return Err(EBUSY);
        }

        let data = KBox::new(FreqData::new(cpu)?, GFP_KERNEL)?;

        policy.set_transition_latency_ns(TRANSITION_LATENCY_NS);

        // NOTE: claiming a 10 us transition latency lets the cpufreq core derive a 15 us
        // governor update interval (`cpufreq_policy_transition_delay_us()` adds 50 %
        // breathing room), i.e. schedutil may retarget 66000 times per second.  On a
        // platform without frequency invariance that turns into governor hunting, and
        // every retarget is a mailbox round trip.  `transition_delay_us` caps the rate
        // the way a mobile governor samples on a fixed interval.
        let delay_us = *module_parameters::transition_delay_us.value();
        if delay_us != 0 {
            policy.set_transition_delay_us(delay_us);
        }

        // Read everything out of `data` that can fail *before* its table is published to
        // the C code, so that an early return cannot leave `policy->freq_table`
        // dangling.
        let suspend_freq = data.default_freq()?;

        // Optional floor for `policy->min`.  The firmware's own minimum (375 MHz on a
        // Loongson-3A6000) is low enough that a hunting governor can leave a core
        // starved for long stretches; a floor keeps it responsive the way a Xeon E3
        // idles around 800 MHz instead of at its lowest P-state.
        //
        // This deliberately does not touch the frequency table: `cpuinfo.min_freq` stays
        // at the firmware minimum so that `lscpu -e` keeps reporting the real hardware
        // range, while `scaling_min_freq` reports the policy floor.
        let min_freq_mhz = *module_parameters::min_freq_mhz.value();
        if min_freq_mhz != 0 {
            let level = data.round_up(min_freq_mhz as usize * 1000)?;

            // SAFETY: `round_up()` returns an index inside the table.
            let index = unsafe { cpufreq::TableIndex::new(level) };

            policy.set_min(data.table.freq(index)?);
        }

        // SAFETY: `FreqData::table` is kept alive by the cpufreq core: it is owned by
        // `data`, which is stored in `policy->driver_data` right after this callback
        // returns, and released in `exit()` — after the core has stopped looking at
        // `policy->freq_table`.
        unsafe { policy.set_freq_table(&data.table) };

        policy.set_suspend_freq(suspend_freq);

        // Every sibling of a physical core shares its frequency domain and therefore its
        // policy.
        sibling_cpumask(cpu).copy(policy.cpus());

        Ok(data)
    }

    fn exit(policy: &mut cpufreq::Policy, data: Option<Self::PData>) -> Result {
        // Put the core back to its default, non-boost level. The C driver ignores the
        // outcome of the request, and so does the cpufreq core for the value returned
        // here, but propagating it keeps the error visible to `exit_callback()`.
        match data {
            Some(data) => set_freq_level(policy, data.default_level),
            None => Ok(()),
        }
    }

    fn online(_policy: &mut cpufreq::Policy) -> Result {
        // Intentionally empty, like the C driver: providing `online`/`offline` makes the
        // cpufreq core tear a policy down "lightly" when its last CPU goes offline, and
        // keep it (and its frequency table) around for a fast resume.
        Ok(())
    }

    fn offline(_policy: &mut cpufreq::Policy) -> Result {
        // See `online()`.
        Ok(())
    }

    fn verify(data: &mut cpufreq::PolicyData) -> Result {
        data.generic_verify()
    }

    fn target_index(policy: &mut cpufreq::Policy, index: cpufreq::TableIndex) -> Result {
        set_freq_level(policy, index.into())
    }

    fn get(policy: &mut cpufreq::Policy) -> Result<u32> {
        // NOTE: the C driver multiplied the `int` returned by the firmware by `KILO` and
        // returned it as an `unsigned int`, so a failed request was reported as a bogus
        // 4.29 THz. Reporting the failure instead makes the cpufreq core handle it as a
        // failed `->get()`, which is what the abstraction expects.
        //
        // NOTE: the C driver addressed `CMD_GET_FREQ_INFO` with the logical CPU number
        // while `CMD_SET_FREQ_INFO` was addressed with `cpu_data[cpu].core`. The two
        // siblings of a core share one frequency domain, so both queries use the core
        // number now and cannot disagree about the same core.
        let freq_raw = smc::request(
            core_of(policy.cpu()),
            smc::FREQ_INFO_TYPE_FREQ,
            smc::CMD_GET_FREQ_INFO,
            0,
            false,
        )?;

        Ok(u32::try_from(freq_to_khz(freq_raw))?)
    }

    fn set_boost(policy: &mut cpufreq::Policy, state: i32) -> Result {
        // `cpufreq_boost_set_sw()` is the generic "boost is a QoS limit" helper the C
        // driver installs; the abstraction does not wrap it yet, so it is called
        // directly.
        //
        // CAST: `cpufreq::Policy` is a `#[repr(transparent)]` wrapper around
        // `Opaque<bindings::cpufreq_policy>`, which is itself transparent.
        let raw: *mut bindings::cpufreq_policy = ptr::from_mut(policy).cast();

        // SAFETY: `raw` points to the live policy the core handed to this callback, and
        // `cpufreq_boost_set_sw()` only uses it for the duration of the call.
        to_result(unsafe { bindings::cpufreq_boost_set_sw(raw, state) })
    }

    fn suspend(policy: &mut cpufreq::Policy) -> Result {
        policy.generic_suspend()
    }
}

impl platform::Driver for Loongson3Cpufreq {
    type IdInfo = ();
    type Data<'bound> = Self;

    fn probe<'bound>(
        pdev: &'bound platform::Device<Core<'_>>,
        _id_info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self, Error> + 'bound {
        smc::init();

        // Refuse to drive an SMC interface we do not know.
        if smc::request(0, 0, smc::CMD_GET_VERSION, 0, false)? == 0 {
            return Err(EPERM);
        }

        // Ask the firmware to enable DVFS and to allow boost levels.
        smc::request(
            smc::FEATURE_DVFS,
            0,
            smc::CMD_SET_FEATURE,
            smc::FEATURE_DVFS_ENABLE | smc::FEATURE_DVFS_BOOST,
            false,
        )?;

        // The registration is owned by devres, so it is dropped — i.e. the driver is
        // unregistered — when the platform device is unbound, exactly where the C
        // driver calls `cpufreq_unregister_driver()` from its `remove()` callback.
        cpufreq::Registration::<Self>::new_foreign_owned(pdev.as_ref())?;

        print_banner();

        Ok(Self)
    }
}

kernel::module_platform_driver! {
    type: Loongson3Cpufreq,
    name: "loongson3_cpufreq",
    authors: ["Huacai Chen <chenhuacai@loongson.cn>"],
    description: "CPUFreq driver for Loongson-3 processors",
    license: "GPL",
    // `MODULE_DEVICE_TABLE(platform, cpufreq_id_table)` in the C driver, whose
    // single entry `{ "loongson3_cpufreq" }` makes modpost emit exactly this
    // alias.  Without it udev cannot auto-load the module when the platform
    // device shows up; the Rust platform abstraction has no `platform_device_id`
    // table yet, so the alias is spelled out.
    alias: ["platform:loongson3_cpufreq"],
    params: {
        min_freq_mhz: u32 {
            default: 0,
            description: "Floor for the policy minimum frequency in MHz, rounded up to a supported level (0 = use the firmware minimum).",
        },
        transition_delay_us: u32 {
            default: 0,
            description: "Minimum time between two frequency changes in microseconds (0 = derive it from the 10 us transition latency). Raising it damps governor-driven frequency hunting.",
        },
    },
}
