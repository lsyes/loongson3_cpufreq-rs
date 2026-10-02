# loongson3_cpufreq

Loongson-3 CPUfreq 驱动的 Rust for Linux 重写。协议层与 C 版逐字节等价；但修复了导致性能损失的缺陷。仅作为独立第三方模块构建，不修改内核。

- 目标：Loongson-3A6000 / loongarch64，内核 `7.2.8+deb14-loong64`
- 原来的C语言驱动：[`c-reference/loongson3_cpufreq.c`](c-reference/loongson3_cpufreq.c)（389 行）
- Rust 驱动：[`loongson3_cpufreq.rs`](loongson3_cpufreq.rs)（727 行，含文档注释）
- 许可证：GPL-2.0-only

**实机效果**：修复前只暴露 224 档中的最低 16 档，固件把请求锁在 937 MHz；修复后稳定运行 2500.9 MHz，boost 下 2600.9 MHz。

## 构建

Rust 模块不能用 `linux-headers` 直接构建：内核 Rust 抽象层（`libkernel.rmeta` 等）不在头文件包里，而且 Rust 符号名带编译器指纹（如 `CsjP4D606FrJC_4core`），rustc 版本不一致会导致 `insmod` 报 `Unknown symbol`。项目提供两个幂等脚本解决：

```sh
sudo apt install linux-headers-$(uname -r)     # 提供 Module.symvers
cd loongson3_cpufreq
tools/setup-rfl.sh                             # 一次性，约 5~10 分钟，免 root
. tools/rfl-env.sh
make && sudo make install
sudo modprobe loongson3_cpufreq
```

`tools/setup-rfl.sh` 会从 `/boot/config-$(uname -r)` 读取内核用的 rustc 版本，下载解包到 `~/.cache/loongson3-cpufreq-rfl`，配置并编译内核 Rust 支持。

交叉/指定内核：

```sh
make KVER=7.2.7+deb14-loong64 KDIR=/path/to/tree
make RUSTC=... BINDGEN=... RUST_LIB_SRC=...
```

DKMS 需显式传入已准备的内核树（`dkms.conf` 刻意无 `AUTOINSTALL`）：

```sh
. tools/rfl-env.sh
dkms build   loongson3-cpufreq/7.2.8 --kernelsourcedir="$KDIR"
dkms install loongson3-cpufreq/7.2.8 --kernelsourcedir="$KDIR"
```

## 修复的缺陷

1. **频率单位错了 16 倍**。固件返回 1/16 MHz 定点数，C 版按 MHz 处理，把 375.9 MHz 报成 6015000 kHz。
2. **档位截断**。`FREQ_MAX_LEVEL = 16` 配真实固件 224 档，只保留 375 MHz 段，`performance` 永远拉不到 2500 MHz。
3. **频率表越界**。`boost_level == 0` 时 `boost_level - 1` 无符号下溢为 `UINT_MAX`；`boost_level > freq_level` 时越过表尾。
4. **空频率表**。`max_level == 0` 时仍构造并索引只有终止项的表。
5. **`get()` 出错返回垃圾频率**。`ret == -EPERM` 时返回约 4.29 THz，且 `cpufreq_online()` 只在 `->cur == 0` 时判失败，会继续用假频率运行。
6. **`get()` 与 `target_index()` 用两套 id**。前者用逻辑 CPU 号，后者用 core 号，同一核的 SMT 线程读写不一致。
7. **频率表缓存悬挂指针**。表缓存在 per-CPU 指针里，内存却由 devres 托管，解绑后 use-after-free。
8. **锁可能锁错对象**。按当前 CPU 的 package 选锁，但请求里的 `id` 可能指向另一 package。

另有若干逐字节对齐的细节（`BOOST_ENABLED=false`、直连 `_printk()` 去掉模块名前缀、显式 `online/offline` 以保持 `->exit()` 调用时机等），详见 `loongson3_cpufreq.rs` 文档注释。

有意保留的差异：一把全局 mailbox 锁（C 版互斥范围的超集）、每次 `->init()` 重新查表、`get()` 失败返回 0。

## 验证

| 项 | 结果 |
| --- | --- |
| `make` | 零警告 |
| crate disambiguator | 与运行内核导出一致 |
| 未解析符号 | 0 个 |
| vermagic / modinfo | 与 C 版模块逐字节相同 |
| `iocsr` 指令序列 | 与 GCC 编译结果一致 |
| SMC 报文编码 | `request(0xa, 0x5, 0x13, 0xbeef, true) == 0x53beef5a` |
| SMC 状态机差分 | `tools/model-test/run.sh` 逐字节相同 |
| C/Rust 实机对拍 | 修复前 `tools/compare-with-c.sh` diff 为空 |

```sh
tools/model-test/run.sh          # 回归测试
sudo tools/install.sh            # 安装并检查 sysfs
```

## 调优

两个模块参数，默认 `0` 保持与 C 版一致：

| 参数 | 说明 |
| --- | --- |
| `min_freq_mhz` | 抬高 `policy->min`（`scaling_min_freq`），向上取到最近真实档位。`800` → 875 MHz。不改频率表，`cpuinfo_min_freq` 仍为 375000 |
| `transition_delay_us` | 换频最短间隔。`0` = 由 10 µs latency 推导（15 µs）；推荐 `1000`（约 3% 单核开销，满载爬升 ~5 ms） |

用 `sudo tools/sweep-rate-limit.sh` 扫出适合本机的值，然后一条命令固化开机配置：

```sh
sudo tools/install-boot-config.sh
```

它写入五个文件并启用一个 oneshot 服务：

| 文件 | 作用 |
| --- | --- |
| `/etc/modules-load.d/loongson3-cpufreq.conf` | 开机加载模块 |
| `/etc/modprobe.d/loongson3-cpufreq.conf` | 上面两个模块参数 |
| `/etc/default/loongson3-cpufreq` | `GOVERNOR` / `BOOST` |
| `/usr/local/sbin/loongson3-cpufreq-apply` | 运行时脚本 |
| `/etc/systemd/system/loongson3-cpufreq.service` | 模块加载后执行该脚本 |

```sh
sudo tools/install-boot-config.sh --min-freq 875 --delay-us 1000 --governor schedutil
sudo tools/install-boot-config.sh --no-boost      # 不开 boost（默认开）
sudo tools/install-boot-config.sh --remove        # 全部撤销
sudo tools/install-boot-config.sh --help
```

**governor 和 boost 必须由服务在模块加载后再设一遍**，两者都挂在随驱动注册而重建的对象上：

- `scaling_governor` 属于 `struct cpufreq_policy`，新 policy 总是从内核默认 governor 开始
  （本机 `CONFIG_CPU_FREQ_DEFAULT_GOV_PERFORMANCE=y`）；
- `cpufreq_driver.boost_enabled`（全局 `/sys/devices/system/cpu/cpufreq/boost`）每次驱动注册
  都回到零初始化的 `false`。

所以**每次重载模块它们都会丢**。`power-profiles-daemon` 在本机是 active/balanced，但走
placeholder 后端、不接管 governor，不会与这个服务冲突。

## 打包

```sh
sudo apt install debhelper dh-dkms
dpkg-buildpackage -b -us -uc
sudo dpkg -i ../loongson3-cpufreq-dkms_7.2.8-2_loong64.deb
```

包不在 postinst 构建模块（stock 系统的 `dkms build` 对 Rust 模块必然失败），只注册并打印指引。内核升级后需重跑 `tools/setup-rfl.sh`。

## 待办

- [ ] 在更多 Loongson-3 机型 / 固件版本上复测。
- [ ] 上游若提供 `set_boost` 包装，移除 `bindings::cpufreq_boost_set_sw` 直调。
- [ ] 上游若提供 topology / IOCSR 抽象，收敛 `cpu_sibling_map` 与内联汇编。
