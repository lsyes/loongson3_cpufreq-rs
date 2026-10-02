/* Model of `do_service_request()` from c-reference/loongson3_cpufreq.c, driven by
 * the same deterministic mock mailbox as model.rs. */
#include <stdio.h>
#include <stdint.h>

typedef uint32_t u32;

/* --- verbatim copy of the C driver's message union --------------------- */
union smc_message {
	u32 value;
	struct {
		u32 id		: 4;
		u32 info	: 4;
		u32 val		: 16;
		u32 cmd		: 6;
		u32 extra	: 1;
		u32 complete	: 1;
	};
};

#define CMD_OK				0
#define LOONGARCH_IOCSR_SMCMBX		0x51c
#define LOONGARCH_IOCSR_MISC_FUNC	0x420
#define IOCSR_MISC_FUNC_SOFT_INT	(1 << 10)

/* --- mock hardware ----------------------------------------------------- */
static u32 mbox, misc;
static u32 reads, sleeps;
static u32 reply_after, reply_status, reply_value;
static int pending;
static u32 written;
static u32 misc_writes[8];
static int misc_write_n;

static void mock_init(int init_complete, u32 after, u32 status, u32 value)
{
	mbox = init_complete ? 0x80000000u : 0u;
	misc = 0; reads = 0; sleeps = 0;
	reply_after = after; reply_status = status; reply_value = value;
	pending = 0; written = 0; misc_write_n = 0;
}

static u32 mock_read(void)
{
	reads++;
	if (pending && reads >= reply_after) {
		mbox = 0x80000000u | ((reply_value & 0xffff) << 8)
				   | ((reply_status & 0x3f) << 24);
		pending = 0;
	}
	return mbox;
}

static void mock_write(u32 v)
{
	written = v; mbox = v; pending = 1; reads = 0;
}

static u32 iocsr_read32(u32 reg)
{
	return reg == LOONGARCH_IOCSR_SMCMBX ? mock_read() : misc;
}

static void iocsr_write32(u32 val, u32 reg)
{
	if (reg == LOONGARCH_IOCSR_SMCMBX)
		mock_write(val);
	else {
		misc_writes[misc_write_n++] = val;
		misc = val;
	}
}

static void usleep_range(u32 min, u32 max) { (void)min; (void)max; sleeps++; }
static void mutex_lock(void) {}
static void mutex_unlock(void) {}

/* --- verbatim copy of do_service_request() ----------------------------- */
static int do_service_request(u32 id, u32 info, u32 cmd, u32 val, u32 extra)
{
	int retries;
	union smc_message msg, last;

	mutex_lock();

	last.value = iocsr_read32(LOONGARCH_IOCSR_SMCMBX);
	if (!last.complete) {
		mutex_unlock();
		return -1; /* -EPERM */
	}

	msg.id		= id;
	msg.info	= info;
	msg.cmd		= cmd;
	msg.val		= val;
	msg.extra	= extra;
	msg.complete	= 0;

	iocsr_write32(msg.value, LOONGARCH_IOCSR_SMCMBX);
	iocsr_write32(iocsr_read32(LOONGARCH_IOCSR_MISC_FUNC) | IOCSR_MISC_FUNC_SOFT_INT,
		      LOONGARCH_IOCSR_MISC_FUNC);

	for (retries = 0; retries < 10000; retries++) {
		msg.value = iocsr_read32(LOONGARCH_IOCSR_SMCMBX);
		if (msg.complete)
			break;

		usleep_range(8, 12);
	}

	if (!msg.complete || msg.cmd != CMD_OK) {
		mutex_unlock();
		return -1; /* -EPERM */
	}

	mutex_unlock();

	return msg.val;
}

static void run(const char *name, int init, u32 after, u32 status, u32 value)
{
	int r;

	mock_init(init, after, status, value);
	r = do_service_request(0xa, 0x5, 0x13, 0xbeef, 1);
	printf("%s: written=0x%08x misc_writes=[", name, written);
	for (int i = 0; i < misc_write_n; i++)
		printf("%s%u", i ? ", " : "", misc_writes[i]);
	printf("] reads=%u sleeps=%u result=%d\n", reads, sleeps, r);
}

int main(void)
{
	run("reply on first poll", 1, 1, 0, 0x1234);
	run("reply on fifth poll", 1, 5, 0, 0xbeef);
	run("error status", 1, 3, 1, 0x0042);
	run("CMD_NOCMD status", 1, 1, 2, 0x0000);
	run("mailbox busy on entry", 0, 1, 0, 0x0000);

	mock_init(1, 0xffffffffu, 0, 0);
	{
		int r = do_service_request(0x1, 0x0, 0x12, 0x0, 0);
		printf("timeout: reads=%u sleeps=%u result=%d\n", reads, sleeps, r);
	}
	return 0;
}
