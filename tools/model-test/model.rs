// Model of `smc::Message` + `smc::request` from loongson3_cpufreq.rs, driven by a
// deterministic mock mailbox.  Mirrors the Rust source line by line.

#[derive(Clone, Copy)]
#[repr(transparent)]
struct Message(u32);

impl Message {
    const fn request(id: u32, info: u32, cmd: u32, val: u32, extra: bool) -> Self {
        Self(
            (id & 0xf)
                | ((info & 0xf) << 4)
                | ((val & 0xffff) << 8)
                | ((cmd & 0x3f) << 24)
                | ((extra as u32) << 30),
        )
    }
    const fn from_raw(value: u32) -> Self { Self(value) }
    const fn into_raw(self) -> u32 { self.0 }
    const fn is_complete(self) -> bool { self.0 & (1 << 31) != 0 }
    const fn status(self) -> u32 { (self.0 >> 24) & 0x3f }
    const fn value(self) -> u32 { (self.0 >> 8) & 0xffff }
}

const EPERM: i32 = -1;

struct Mock {
    mbox: u32,
    misc: u32,
    reads: u32,
    sleeps: u32,
    // script
    reply_after: u32,
    reply_status: u32,
    reply_value: u32,
    pending: bool,
    written: u32,
    misc_writes: Vec<u32>,
}

impl Mock {
    fn new(init_complete: bool, reply_after: u32, reply_status: u32, reply_value: u32) -> Self {
        Self {
            mbox: if init_complete { 0x8000_0000 } else { 0 },
            misc: 0,
            reads: 0,
            sleeps: 0,
            reply_after,
            reply_status,
            reply_value,
            pending: false,
            written: 0,
            misc_writes: Vec::new(),
        }
    }
    // `iocsr_read32(IOCSR_SMCMBX)`
    fn read(&mut self) -> u32 {
        self.reads += 1;
        if self.pending && self.reads >= self.reply_after {
            self.mbox = 0x8000_0000
                | ((self.reply_value & 0xffff) << 8)
                | ((self.reply_status & 0x3f) << 24);
            self.pending = false;
        }
        self.mbox
    }
    // `iocsr_write32(msg, IOCSR_SMCMBX)`
    fn write(&mut self, v: u32) {
        self.written = v;
        self.mbox = v;
        self.pending = true;
        self.reads = 0;
    }
    // `iocsr_read32(IOCSR_MISC_FUNC)` / `iocsr_write32(v, IOCSR_MISC_FUNC)`
    fn read_misc(&self) -> u32 { self.misc }
    fn write_misc(&mut self, v: u32) { self.misc_writes.push(v); self.misc = v; }
    // `usleep_range(8, 12)`
    fn sleep(&mut self, _min: u32, _max: u32) { self.sleeps += 1; }
}

fn request(id: u32, info: u32, cmd: u32, val: u32, extra: bool, m: &mut Mock) -> Result<u32, i32> {
    let last = Message::from_raw(m.read());
    if !last.is_complete() {
        return Err(EPERM);
    }
    let msg = Message::request(id, info, cmd, val, extra);
    m.write(msg.into_raw());
    let misc_func = m.read_misc();
    m.write_misc(misc_func | (1 << 10));

    for _ in 0..10_000 {
        let reply = Message::from_raw(m.read());
        if reply.is_complete() {
            return if reply.status() == 0 {
                Ok(reply.value())
            } else {
                Err(EPERM)
            };
        }
        m.sleep(8, 12);
    }
    Err(EPERM)
}

fn main() {
    let cases: [(bool, u32, u32, u32, &str); 5] = [
        (true, 1, 0, 0x1234, "reply on first poll"),
        (true, 5, 0, 0xbeef, "reply on fifth poll"),
        (true, 3, 1, 0x0042, "error status"),
        (true, 1, 2, 0x0000, "CMD_NOCMD status"),
        (false, 1, 0, 0x0000, "mailbox busy on entry"),
    ];
    for (init, after, status, value, name) in cases {
        let mut m = Mock::new(init, after, status, value);
        let r = request(0xa, 0x5, 0x13, 0xbeef, true, &mut m);
        println!(
            "{}: written=0x{:08x} misc_writes={:?} reads={} sleeps={} result={}",
            name,
            m.written,
            m.misc_writes,
            m.reads,
            m.sleeps,
            match r {
                Ok(v) => v as i32,
                Err(e) => e,
            }
        );
    }
    // Timeout case: lowered retry count is not modelled, so just check the bounds.
    let mut m = Mock::new(true, u32::MAX, 0, 0);
    let r = request(0x1, 0x0, 0x12, 0x0, false, &mut m);
    println!(
        "timeout: reads={} sleeps={} result={}",
        m.reads,
        m.sleeps,
        match r {
            Ok(v) => v as i32,
            Err(e) => e,
        }
    );
}
