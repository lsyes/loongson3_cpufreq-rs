# loongson3_cpufreq — Loongson-3 CPU 调频驱动的 Rust for Linux 重写

把内核源码树里的 `drivers/cpufreq/loongson3_cpufreq.c` 用 **Rust for Linux**
改写。**协议层与 C 版逐字节等价**（SMC 报文、重试、错误码、回调语义，已用差分
测试和实机对拍证明），但**策略层修正了 C 版会实际损害性能的缺陷**；仍然作为
**独立第三方内核模块**构建，不改动内核。

实机结果（Loongson-3A6000，标称 2500 MHz）：修复前驱动只暴露 224 档中的最低
16 档（375 MHz 段），固件把请求钳在 937 MHz；修复后 CPU 运行在 2500.9 MHz，
boost 下可达 2600.9 MHz。**提升 2.67 倍**。

- 内核：`7.2.8+deb14-loong64`（Debian `7.2.8-1`，loongarch64）
- 原 C 版：md5 `982167cec8d3076121cb92af23c9448f`，389 行，原样保存在
  [`c-reference/loongson3_cpufreq.c`](c-reference/loongson3_cpufreq.c)
- Rust 版：[`loongson3_cpufreq.rs`](loongson3_cpufreq.rs)，605 行（含文档注释）
- 许可证：GPL-2.0-only（与内核一致，模块需要 `EXPORT_SYMBOL_GPL` 的符号）

## 目录结构

```
loongson3_cpufreq/
├── loongson3_cpufreq.rs                        # Rust 驱动（本次重写的产物）
├── c-reference/loongson3_cpufreq.c             # 原 C 驱动，仅作对照，不参与构建
├── Makefile                                    # 外部模块构建（kbuild 双用途）
├── dkms.conf                                   # 手工 dkms 用
├── tools/
│   ├── setup-rfl.sh                            # 一次性准备内核树 + 工具链（免 root）
│   ├── rfl-env.sh                              # 导出 RUSTC/BINDGEN/KDIR 等环境
│   └── model-test/                             # SMC 状态机 C/Rust 差分测试
├── README.md
└── debian/                                     # Debian 打包
```

## 一、为什么要准备工具链（先看这段）

C 版可以直接用 DKMS / `linux-headers` 构建，**Rust 版不行**，两个独立的原因：

1. **Rust 模块链接的是内核里编译好的 Rust 抽象层**，即
   `rust/libkernel.rmeta`、`rust/libcore.rmeta`、`rust/bindings/*` 等。
   `linux-headers-*` 包不含这些（发行版只为 C 模块准备外部构建环境）。

2. **Rust 符号名里带编译器指纹。** 每个被 mangle 的符号长这样：

   ```
   _RNvNtCsjP4D606FrJC_4core9panicking9panic_fmt
            ^^^^^^^^^^^^^ 这一截是 crate disambiguator
   ```

   它由 rustc 版本 + 编译选项共同哈希得到。内核导出的是
   `CsjP4D606FrJC_4core` / `Cs1L1xvvvYXuH_6kernel`，如果模块用别的 rustc
   编译，就会引用 `CsXXXXXXXXXXX_4core` 这种内核里根本不存在的符号，
   `insmod` 会直接 `Unknown symbol`。

   实测：用 rustup 的 `1.95.0`（版本号相同，但 LLVM 是 22.1.2、Debian 的是
   21.1.8）会得到 `Cs1peUGmbrgHn_4core`，**加载必失败**；换成 Debian 自己的
   `rustc 1.95.0 (59807616e 2026-04-14) (built from a source tarball)` 之后
   哈希与内核完全一致。

所以本项目附带两个脚本：

- [`tools/setup-rfl.sh`](tools/setup-rfl.sh)：读取 `/boot/config-$(uname -r)`
  里的 `CONFIG_RUSTC_VERSION_TEXT` 找出内核用的 rustc 版本，用
  `apt-get install --download-only`（**只下载不安装，不需要 root**）把对应版本的
  `rustc` / `rust-src` / `libstd-rust-dev` / `bindgen` / `libclang-dev` 解包到
  `~/.cache/loongson3-cpufreq-rfl/root`，再把 `linux-source-<主>.<次>` 解包、
  用运行内核的 `.config` 配置好，编译内核自己的 Rust 支持
  （`make prepare modules_prepare`），最后把运行内核的 `Module.symvers` 拷进树里。
- [`tools/rfl-env.sh`](tools/rfl-env.sh)：把上一步生成的
  `$CACHE/env.sh` 载入当前 shell。

脚本是幂等的，第二次运行只做检查。缓存默认在
`~/.cache/loongson3-cpufreq-rfl`，可用 `RFL_CACHE` 改。

## 二、构建

```sh
sudo apt install linux-headers-$(uname -r)     # 提供 Module.symvers
cd loongson3_cpufreq
tools/setup-rfl.sh                             # 一次性，约 5~10 分钟
. tools/rfl-env.sh
make                                           # 生成 loongson3_cpufreq.ko
sudo make install
sudo modprobe loongson3_cpufreq
```

`make check-env` 会检查内核树是否已准备好、`include/config/kernel.release`
是否与运行内核一致、以及 rustc/bindgen 是否可用，任何一项不满足都会给出可操作
的提示而不是编译到一半报错。

交叉或指定内核：

```sh
make KVER=7.2.7+deb14-loong64 KDIR=/path/to/prepared/tree
make RUSTC=/path/to/rustc BINDGEN=/path/to/bindgen RUST_LIB_SRC=/path/to/rust/library
```

DKMS 也可以用，但要显式把准备好的树喂给它（`dkms.conf` 里刻意**没有**写
`AUTOINSTALL`，免得在 stock 系统上反复失败）：

```sh
. tools/rfl-env.sh
dkms build   loongson3-cpufreq/7.2.8 --kernelsourcedir="$KDIR"
dkms install loongson3-cpufreq/7.2.8 --kernelsourcedir="$KDIR"
```

## 三、行为对照

### 3.1 一一对应关系

| C 版 | Rust 版 | 说明 |
| --- | --- | --- |
| `union smc_message` | `smc::Message` | 不再依赖 C 位域，改成显式移位，布局与 GCC 在小端下的结果逐位一致：`id[3:0] info[7:4] val[23:8] cmd[29:24] extra[30] complete[31]` |
| `do_service_request()` | `smc::request()` | 命令字、请求序列、重试次数、错误码完全相同 |
| `iocsr_read32/write32()` | `unsafe fn iocsr_read32/write32()` | GCC 用内建函数，Rust 用 `asm!("iocsrrd.w {0}, {1}")`；反汇编确认生成的指令与 C 版逐字节一致 |
| `usleep_range(8, 12)` | `bindings::usleep_range_state(8, 12, TASK_UNINTERRUPTIBLE)` | Rust 抽象层只有忙等的 `udelay()`，这里直连 C 符号（内核已导出） |
| `cpu_data[cpu].package/.core` | `cpu_info()` / `core_of()` | 通过绑定的零长数组 `bindings::cpu_data` 取 |
| `topology_sibling_cpumask()` | `sibling_cpumask()` | 宏展开成 `&cpu_sibling_map[cpu]`，Rust 直接取 `bindings::cpu_sibling_map` |
| `cpufreq_mutex[MAX_PACKAGES]` | `smc::MAILBOX` | 见 3.3，这是有意的简化 |
| `loongson3_cpufreq_get()` | `cpufreq::Driver::get` | |
| `loongson3_cpufreq_target()` | `cpufreq::Driver::target_index` | |
| `configure_freq_table()` | `FreqData::new()` | |
| `loongson3_cpufreq_cpu_init()` | `cpufreq::Driver::init` | 转换延迟 10000 ns、频率表、`suspend_freq`、sibling cpumask 顺序与 C 一致 |
| `loongson3_cpufreq_cpu_exit()` | `cpufreq::Driver::exit` | 回落到默认（非 boost）档 |
| `cpu_online/offline` 空实现 | `Driver::online/offline` 返回 `Ok(())` | **必须实现**：只要注册了 `online`/`offline`，cpufreq 核心才会走"轻量下线"路径、把 `->exit()` 推迟到策略真正销毁时；省掉它们会改变 `->exit()` 的调用时机 |
| `.verify = cpufreq_generic_frequency_table_verify` | `PolicyData::generic_verify()` | |
| `.suspend = cpufreq_generic_suspend` | `Policy::generic_suspend()` | |
| `.set_boost = cpufreq_boost_set_sw` | `bindings::cpufreq_boost_set_sw()` | 抽象层还没包这个函数，直接调导出符号 |
| `.flags = CPUFREQ_CONST_LOOPS` | `cpufreq::flags::CONST_LOOPS` | |
| `.name = "loongson3"` | `const NAME = c"loongson3"` | |
| `module_platform_driver()` | `kernel::module_platform_driver!` | 模块名 `loongson3_cpufreq`，与平台设备名匹配；`pr_info!` 前缀同 C 的 `KBUILD_MODNAME` |
| `MODULE_DEVICE_TABLE(platform, cpufreq_id_table)` | `alias: ["platform:loongson3_cpufreq"]` | 必须显式写：Rust 平台抽象还没有 `platform_device_id` 表，缺了 udev 就无法按 modalias 自动加载 |
| `devm_kzalloc(freq_data)` | `cpufreq::TableBox` 放进 `PData`（`KBox<FreqData>`） | 由 `policy->driver_data` 持有，生命周期正好覆盖 `policy->freq_table` 的使用期 |
| `devm_mutex_init()` × 16 | `smc::init()` 里初始化一把 `GlobalLock` | |
| `pr_info("cpufreq: ...")` | `print_banner()`（直连 `_printk()`） | C 文件没有定义 `pr_fmt()`，所以 dmesg 里**没有**模块名前缀；而 RfL 的 `pr_info!` 强制加 `ModuleMetadata::NAME`。这条差异是 `tools/compare-with-c.sh` 的 diff 抓出来的，改成直连 `_printk()` 后两边逐字节相同 |
| `devm_*` 托管释放 | `devres::register()` 托管 `cpufreq::Registration` | 平台设备 unbind 时自动 `cpufreq_unregister_driver`，与 C 的 `remove()` 回调同一时机 |

另外 `BOOST_ENABLED` 特意取 `false`：这个字段对应的是
`cpufreq_driver.boost_enabled`（**当前全局 boost 状态**，不是"驱动是否支持
boost"）。C 版靠零初始化把它留成 `false`，所以
`/sys/devices/system/cpu/cpufreq/boost` 初始读到 `0`，per-policy 的 `boost`
也要等全局开关打开后才可写。如果这里填 `true`，策略创建时会立刻调用
`cpufreq_boost_set_sw(policy, 1)` 把 boost 打开，行为就和 C 版不一样了。

### 3.2 修掉的缺陷

1. **频率表越界（原 C 第 263 行、第 294 行）**

   ```c
   data->def_freq_level = boost_level - 1;                       /* 无符号下溢 */
   policy->suspend_freq = policy->freq_table[...def_freq_level].frequency;
   ```

   固件返回 `boost_level == 0` 时 `def_freq_level` 变成 `UINT_MAX`，
   返回 `boost_level > freq_level` 时又越过表尾 —— 两种情况都在读表外内存。
   Rust 版：

   ```rust
   let default_level = (boost_level.saturating_sub(1) as usize).min(levels - 1);
   ```

   正常范围内结果与 C 完全一致（`boost_level - 1`）。

2. **空频率表**

   `max_level == 0` 时 C 版会构造一张只有终止项的表，然后照样索引它。
   Rust 版直接 `Err(ENODEV)`，让 `cpufreq_online` 干净地失败。

3. **频率单位错了 16 倍（原 C 第 226、272 行）**

   固件返回的频率是 **1/16 MHz（62.5 kHz）定点数**，不是 MHz。实机 dump 证据：
   3A6000 的 2500 MHz 标称档回 `40000`（= 2500×16），2600 MHz 的 boost 块回
   `41600`，375 MHz 块从 `6000` 起。C 版 `ret * KILO` 把每个数字都放大 16 倍，
   于是 375.9 MHz 被报成 `6015000` kHz —— 这就是"6 GHz"的来历。
   Rust 版用 `freq_to_khz()`（`val * 125 / 2`）换算。

4. **档位被截断到 224 档中的最低 16 档（原 C 第 165、258 行）**

   `FREQ_MAX_LEVEL = 16` 配上真实固件的 224 档，只留下 375.0–375.9 MHz 那一段，
   `performance` governor 因此永远拉不到 2500 MHz（实测固件把请求钳在 937 MHz，
   约为标称的 37%）。Rust 版使用固件报告的全部 224 档，`FREQ_MAX_LEVEL` 只作为
   防爆上限保留。

5. **`get()` 与 `target_index()` 用了两套 id（原 C 第 224、233 行）**

   `CMD_SET_FREQ_INFO` 用 `cpu_data[cpu].core`，`CMD_GET_FREQ_INFO` 却用逻辑 CPU 号。
   同一个核的两个 SMT 线程共享频率域，两套 id 会让"设"和"读"对不上。Rust 版统一
   用 core 号。

6. **`get()` 出错时返回垃圾频率（原 C 第 220~227 行）**

   ```c
   int ret = do_service_request(...);
   return ret * KILO;      /* ret == -EPERM(-1) 时返回 4294966296，约 4.29 THz */
   ```

   不仅数值荒谬，而且 `cpufreq_online()` 只在 `->cur == 0` 时才判定 `->get()`
   失败，所以 C 版会把 4.29 THz 当成当前频率继续跑。Rust 版返回
   `Err(EPERM)`，抽象层把它转成 0，核心据此报 `->get() failed`。

7. **频率表缓存的悬挂指针（原 C 第 174/245/279/299 行）**

   C 版把表缓存在 per-CPU 指针 `freq_data[]` 里，内存却由 devres 托管。设备
   解绑后内存释放，per-CPU 指针仍指向已释放区域；下次 probe 之前若有人访问就是
   use-after-free。Rust 版让表与策略同生共死，每次 `->init()` 重新向固件查询
   （见 3.3）。

8. **包号索引的锁可能锁错对象（原 C 第 179~183 行）**

   C 版按**当前 CPU** 的 package 选锁，但请求里的 `id` 可能指向另一个 package
   的核心（`target_index` 传的是 `cpu_data[policy->cpu].core`）。Rust 版用一把
   全局锁，是 C 版互斥范围的严格超集。

### 3.3 有意保留的差异

| 差异 | 理由 |
| --- | --- |
| 一把全局 mailbox 锁，而不是 16 把按 package 索引的锁 | 见 3.2 第 5 条；语义上只多不少。SMC 事务本身很短（µs 级），且固件侧大概率本来就是串行的 |
| 每次 `->init()` 重新查询频率表，不复用缓存 | 见 3.2 第 4 条；固件是频率表的唯一权威，重新查询得到同样的值。CPU 热插拔导致策略整体重建时才会发生，代价是几次 SMC 往返 |
| `get()` 失败返回 0 而不是 4.29 THz | 见 3.2 第 3 条，由抽象层的 `Result<u32>` 语义决定 |

除此之外，命令字、参数、请求顺序、重试与休眠节奏、返回码、频率表内容、
boost 标记、`suspend_freq`、sibling cpumask、驱动名与 flags 都与 C 版一致。

## 四、实机验证（Loongson-3A6000，固件 224 档 / boost=208）

| 指标 | 修复前 | 修复后 |
| --- | --- | --- |
| `scaling_available_frequencies` | 16 档 `6000000…6015000`（假值） | **208 档 `375000…2500937`**（真实 kHz） |
| `cpuinfo_cur_freq`（固件实测） | `14992000`，实际 **937 MHz** | **`2500937`**，实际 **2500.9 MHz** |
| boost 档 | 从未生效（`boost_level=208` 溢出 16 档表） | **`2600937` 生效** |
| CPU 实际频率 | 标称的 **37.5 %** | **100 %**（boost 下 104 %） |
| `def_freq_level = boost_level-1` | 越界读约 2.3 KB | `207`，正好是最高非 boost 档 |
| dmesg 前缀 | 多一个 `loongson3_cpufreq:` | 与 C 版一致 |

固件原始回复（`tools/smc-dump/`）与解码：

| 命令 | 原始 `val` | 含义 |
| --- | --- | --- |
| `CMD_GET_FREQ_LEVEL_NUM` | `224` | 14 个频率块 × 16 个子步 |
| `CMD_GET_FREQ_BOOST_LEVEL` | `208` | boost 块（2600 MHz）的起点 |
| `CMD_GET_FREQ_LEVEL_INFO(0)` | `6000` | 375.0 MHz |
| `CMD_GET_FREQ_LEVEL_INFO(207)` | `40015` | 2500.9375 MHz（最高非 boost） |
| `CMD_GET_FREQ_LEVEL_INFO(208)` | `41600` | 2600.0 MHz（boost） |
| `CMD_GET_FREQ_INFO(core 0)` | `14992` → 现 `40009` | 937 MHz → 2500.6 MHz |

单位是 **1/16 MHz（62.5 kHz）**：`40000 = 2500 × 16` 精确对应 3A6000 标称频率。

## 五、已完成的验证

在 LoongArch（loong64）本机、内核 `7.2.8+deb14-loong64` 上：

| 验证项 | 结果 |
| --- | --- |
| `make` 构建 | ✓ 生成 `loongson3_cpufreq.ko`，**零编译警告**（内核打开 `-Wmissing_docs -Wrust_2018_idioms -Wunreachable_pub -Dunsafe_op_in_unsafe_fn` 等） |
| crate disambiguator | ✓ `core` = `CsjP4D606FrJC_`、`kernel` = `Cs1L1xvvvYXuH_`，与运行内核导出的符号完全一致 |
| 未解析符号 | ✓ 模块全部未定义符号都能在运行内核的 `Module.symvers` 中找到（0 个 unresolved） |
| vermagic | ✓ `7.2.8+deb14-loong64 SMP preempt mod_unload LOONGARCH 64BIT`，与 C 版模块逐字节相同 |
| `modinfo` | ✓ `name=loongson3_cpufreq`、`license=GPL`、`alias=platform:loongson3_cpufreq`、author/description 与 C 版逐项一致 |
| 指令序列 | ✓ `iocsr_read32/write32` 生成的 `iocsrrd.w` / `iocsrwr.w` 与 GCC 编译 C 版的结果一致 |
| SMC 报文编码 | ✓ 单独编译运行验证：`request(0xa, 0x5, 0x13, 0xbeef, true) == 0x53beef5a`，字段越界时按位截断，与 C 位域一致 |
| SMC 状态机差分测试 | ✓ `tools/model-test/run.sh`：C 版 `do_service_request()` 与 Rust 版 `smc::request()` 的逐行转写，跑同一组 mock 邮箱场景（正常/延迟回复/错误状态/超时/邮箱忙），报文内容、`MISC_FUNC` 写入、轮询次数、`usleep_range` 次数、返回值**逐字节相同** |
| 平台设备就绪 | ✓ `/sys/bus/platform/devices/loongson3_cpufreq` 存在，驱动名匹配 |
| **C/Rust 实机对拍** | ✓ 修复前 `sudo tools/compare-with-c.sh` 快照 104 行 sysfs 状态 **diff 为空**；修复频率单位与档位截断后**必然有差异**，这是预期的（见第四节） |
| 实机 SMC dump | ✓ `tools/smc-dump/` 打出固件原始回复：224 档 / boost=208 / 单位 1/16 MHz |

**已做**：`./tools/install.sh` 实机加载通过，`/sys/.../cpu0/cpufreq/` 全部就位。
下面是复查用的命令：

```sh
sudo tools/install.sh
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver          # 期望 loongson3
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_available_frequencies
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq
cat /sys/devices/system/cpu/cpu0/cpufreq/boost                   # 期望 0（与 C 版一致）
dmesg | grep -i "Loongson-3 CPU frequency"
sudo modprobe -r loongson3_cpufreq                               # 卸载应无告警
```

对照实验建议：把 C 版模块和 Rust 版模块分别加载，diff 上述 sysfs 输出与
`cpufreq-info`（若安装）的结果。

## 六、调优：下限与换频阻尼

驱动有两个模块参数（默认 `0` = 保持与 C 版一致的行为）：

| 参数 | 默认 | 作用 |
| --- | --- | --- |
| `min_freq_mhz` | `0` | 抬高 `policy->min`（即 `scaling_min_freq`），向上取到最近的**真实档位**。`800` → 875 MHz（本机档位为 375/500/625/**875**/1125/1312/1500/1875/…）。**不改频率表** |
| `transition_delay_us` | `0` | 两次换频之间的最短间隔。`0` 表示由 10 µs 的 `transition_latency` 推导（= 15 µs）。值越大越省邮箱往返，但**满载爬升越慢** |

### 硬件能力 vs 策略窗口

这两个概念在 sysfs 里是分开的，不要混淆：

| | 来源 | 含义 | 本机（`min_freq_mhz=800`） |
| --- | --- | --- | --- |
| `cpuinfo_min_freq` | 频率表（`cpufreq_frequency_table_cpuinfo()`） | **硬件能力** | `375000` |
| `scaling_min_freq` | `policy->min` | **策略下限** | `875000` |

`lscpu -e` 的 `MINMHZ`/`MAXMHZ` 读的是 `cpuinfo_*`，即**硬件范围**，所以配了下限之后它仍然显示 `375` —— 这是**正确**的，固件确实支持 375 MHz 那一档。`cpupower frequency-info` 也把两者分开显示（`hardware limits` vs `current policy`）。

`min_freq_mhz` 只动 `policy->min`，**不会**把低频档从表里删掉，理由有两条：

1. 硬件能力是事实，不该为了显示好看而裁剪描述；
2. 375–875 MHz 那段在执行"省电优先"的任务时是真有用的，裁掉就只能改参数重载才能拿回来。

`policy->min` 已经能保证 governor 不会选到它以下的档位，功能上足够。

**爬升延迟的换算**：schedutil 每次评估按 `util/capacity` 折算目标，`map_util_freq()` 相对当前频率约 **×1.25 一步**，从 875 MHz 到 2600 MHz 约 5 步，所以

| `transition_delay_us` | 换频上限 | 满载爬升 | SMC 邮箱开销 |
| --- | --- | --- | --- |
| `0`（15 µs） | 66000/s | ~0.1 ms | ~9% 一个核，且 governor 抖动（实测 ~2900 次/秒） |
| **`1000`（推荐）** | 1000/s | ~5 ms | ~3% |
| `5000` | 200/s | ~25 ms | ~0.6% |
| `10000` | 100/s | **~50 ms** | ~0.3%，交互可感 |

用 [`tools/sweep-rate-limit.sh`](tools/sweep-rate-limit.sh) 扫出适合本机的值：

```sh
sudo tools/sweep-rate-limit.sh              # 默认扫 200 500 1000 2000 5000
sudo tools/sweep-rate-limit.sh 100 500 1000 # 或指定
```

它逐个设置 `schedutil/rate_limit_us`（运行时 tunable，**不用重编**），对同一物理核的两个 SMT 线程加 3 秒负载，打印换频次数和频率分布。

选好之后固化：

```sh
echo 'options loongson3_cpufreq min_freq_mhz=800 transition_delay_us=1000' \
  | sudo tee /etc/modprobe.d/loongson3-cpufreq.conf
```

### 开机自动生效

本机内核默认 governor 是 `performance`（`CONFIG_CPU_FREQ_DEFAULT_GOV_PERFORMANCE=y`），而且**每次重载模块 governor 都会重置回 `performance`**（`scaling_governor` 挂在随模块重建的 `policy` 上）。`power-profiles-daemon` 虽然 active，但在这台机器上走 placeholder 后端，不接管 governor。所以需要显式配置：

```sh
# 开机加载模块
echo loongson3_cpufreq | sudo tee /etc/modules-load.d/loongson3-cpufreq.conf

# 模块参数
echo 'options loongson3_cpufreq min_freq_mhz=800 transition_delay_us=1000' \
  | sudo tee /etc/modprobe.d/loongson3-cpufreq.conf

# 加载后切 schedutil
sudo tee /etc/systemd/system/loongson3-cpufreq.service >/dev/null <<'UNIT'
[Unit]
Description=Loongson-3 cpufreq: load the driver and select schedutil
After=systemd-modules-load.service
Before=multi-user.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/sbin/modprobe loongson3_cpufreq
ExecStart=/bin/sh -c 'for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do echo schedutil > "$g"; done'

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload && sudo systemctl enable --now loongson3-cpufreq.service
```

注：模块带了 `alias: ["platform:loongson3_cpufreq"]`，`modules.alias` 里也有对应条目，但本机 udev 并没有通用的 modalias→modprobe 规则，所以不要依赖 udev 自动加载，用 `modules-load.d` 更可靠。

## 七、回归测试

```sh
tools/model-test/run.sh
```

[`tools/model-test/model.c`](tools/model-test/model.c) 是 C 版 `do_service_request()`
的逐行转写（连 `union smc_message` 位域都原样保留），
[`tools/model-test/model.rs`](tools/model-test/model.rs) 是 Rust 版 `Message` +
`smc::request()` 的逐行转写。两者由同一个确定性 mock 邮箱驱动，输出直接 `diff`。
这是在不加载模块的前提下，对协议层做的最强等价性验证。

## 八、打包说明

- [`debian/README.Debian`](debian/README.Debian) 解释了为什么这个包**不在
  postinst 里构建模块**：stock 系统上 `dkms build` 走的是 `linux-headers`，
  对 Rust 模块必然失败，而且会在每次内核升级时重试一遍。
  `dkms.conf` 里因此刻意不写 `AUTOINSTALL`，postinst 只注册并打印指引。
- 包内安装 `/usr/src/loongson3-cpufreq-7.2.8/`：Rust 源码、`Makefile`、
  `tools/`、`c-reference/`、`dkms.conf`、`README.md`。
- 每次内核升级都需要为新内核重跑一次 `tools/setup-rfl.sh` —— Rust 符号哈希和
  内核 ABI 都随版本变化。

构建 Debian 包：

```sh
sudo apt install debhelper dh-dkms
cd loongson3_cpufreq
dpkg-buildpackage -b -us -uc
sudo dpkg -i ../loongson3-cpufreq-dkms_7.2.8-2_loong64.deb
```

## 九、升级 / 改版本时要同步的地方

1. `debian/changelog` 顶部版本号（如 `7.2.8-3`）；
2. `debian/loongson3-cpufreq-dkms.install` 与 `debian/README.Debian` 里的
   `usr/src/loongson3-cpufreq-<新版本>/`；
3. `dkms.conf` / `debian/loongson3-cpufreq-dkms.dkms` 里的 `PACKAGE_VERSION`
   （仅供手工 dkms 使用，与打包无关）。

## 十、参考资料

Rust for Linux 的资料确实偏少，本次重写是直接读源码确定的 API，主要依据：

| 内容 | 文件 |
| --- | --- |
| cpufreq 抽象（`Driver`/`Policy`/`PolicyData`/`Table*`/`Registration`） | `rust/kernel/cpufreq.rs` |
| 平台驱动与 `module_platform_driver!` | `rust/kernel/platform.rs`、`rust/kernel/driver.rs` |
| devres 托管注册 | `rust/kernel/devres.rs` |
| 全局锁 `global_lock!` | `rust/kernel/sync/lock/global.rs` |
| cpumask、CpuId | `rust/kernel/cpumask.rs`、`rust/kernel/cpu.rs` |
| bindgen 允许清单（哪些 C 符号能直接用） | `rust/bindings/bindings_helper.h`、`rust/helpers/` |
| cpufreq 核心生命周期（决定 `->init/->exit/->online/->offline` 调用时机） | `drivers/cpufreq/cpufreq.c` |
| LoongArch IOCSR / SMC | `arch/loongarch/include/asm/loongarch.h`、`arch/loongarch/power/platform.c` |

## 待办

- [ ] 在有 root 的机器上完成 `insmod` / sysfs 对照实测（见第四节）。
- [ ] 如果上游 `rust/kernel/cpufreq.rs` 后续提供 `set_boost` 的包装，去掉对
      `bindings::cpufreq_boost_set_sw` 的直接调用。
- [ ] 如果上游提供 topology / IOCSR 的 Rust 抽象，收敛 `bindings::cpu_sibling_map`
      与内联汇编这两处。
