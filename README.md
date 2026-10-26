# nv-uefi-unlocker

UEFI 预驱动 NVIDIA GPU 解锁器（Windows，Rust）。在 NVIDIA 驱动加载**之前**的
UEFI DXE 时窗执行完整固件事务：SEC2 scrubber → FWSEC-FRTS（WPR2）→ 签名
Booter 启动 GSP-RM →（门控后）V67 四写 payload → 读回验证 → FLR → 链式交回
Windows Boot Manager。

**当前状态：M4 闭环。** 已在 RTX 4060 Laptop（`10de:28e0`，AD107M）真机完成
四写解锁并双端验证——固件侧 marker `0x67b00001` + 四 PLM 读回，Windows 侧
FP16 GEMM 29.89 → 49.33 TFLOPS（1.65×）。完整报告见
[docs/ada28e0-uefi-report.md](docs/ada28e0-uefi-report.md)（bar1 报告格式：
判据表、链条 15 步、关键修复、社区致谢）。

该卡此前未被社区验证（最近的公开 canary 是 28e1，同芯片 ASUS 移动端）。

## 使用（RTX 40 系笔记本/台式，AD10x 家族）

1. 构建：`cargo efi-tx`（gated 事务构建；见下方构建节）。
2. U 盘 FAT32 放入：`loader.efi` 改名 `BOOTX64.EFI`（先备份 U 盘原有文件），
   以及 `assets/out/` 的 `booter_load_ad102.bin`、`gsp_boot_desc_ad102.bin`、
   `gsp_boot_ad102.bin`、`fwimage_ad102.bin`、`fwsignature_ad102.bin`、
   `scrubber_ad10x.bin`；**解锁模式再加** `v67-ad102-four-plm.bin`（不放即为
   纯回放/诊断模式）。
3. BIOS 关 Secure Boot、独显直连（dGPU only）模式；从 U 盘启动。
4. 空格确认 → 全自动 → 日志落 `\loader.log` → 30 秒无操作自动回 Windows。

解锁为**易失**性质：每次冷启动重放一次，无持久化、无刷写、断电即清。
V67 payload 生成：`cargo run -p assets -- generate-ad102`（从钉 SHA-256 的
610.43.03 公开输入本地再生，产物 hash 硬校验）。

## 与 nvoc kmd 功耗墙解锁配合使用

本工具与 **[Skyworks-Neo/nvoc](https://github.com/Skyworks-Neo/nvoc)** 的 kmd
功耗墙解锁**兼容且正交**：本工具在 UEFI 预驱动窗口解除 tensor 发射速率钳制
（FEAT/WPR/WPRCFG PLM），nvoc kmd 在操作系统内解除功耗/电压墙——两者作用于
不同层级，可叠加，推荐配合使用以获得极限性能（实测组合：4060 Laptop 上
FP16 GEMM 8192 达 60–63 TFLOPS，芯片功耗 ~164 W，远超原厂 TGP）。

> **⚠️ 硬件风险警告**
> 两项解锁叠加后芯片功耗与热密度会大幅超出原厂设计包络（实测 Hot Spot
> 可达 104 °C、PerCap Reason=Pwr 持续）。使用前必须自行确认并承担：
> **散热系统足以应对**（液金/硅脂状态、风扇策略、均热板与热管规格），
> **供电链路足以承载**（适配器功率、VRM 余量、电池策略），并保持监控
> （温度/功耗/PerCap Reason）。**因超频超压超温导致的芯片、供电或整机
> 物理损坏，本项目概不负责**——解锁是易失的，但烧坏的硅片不会恢复。

## 仓库结构

```
loader/    EFI 应用 crate（no_std, x86_64-unknown-uefi, Rust + uefi-rs）
assets/    host 侧资产管线（钉 hash 下载/校验/抽取/载荷生成，纯 Rust）
sim/       行为级模拟器 + 事务状态机测试（无 GPU 开发）
docs/      报告、里程碑证据（evidence-as-commits）、寄存器冷值表、开放问题
```

## 安全宪法（副本，与 AGENTS.md §1 等义，违反任何一条即停工）

1. **读回是唯一成功判据**。状态码/返回值/中断/自检输出一律不构成执行证明。
2. **STOP/异常后同会话绝不重试**；完全断电后走正常 Windows 引导恢复。
3. 只读链式加载 ESP 的 `\EFI\Microsoft\Boot\bootmgfw.efi`，**永不覆写 ESP
   任何文件**；替换 U 盘 BOOTX64.EFI 前先备份原有。
4. 无常驻引导项：只从 U 盘/启动菜单一次性进入；不动 NVRAM BootOrder。
5. 每张卡钉死身份（PCI vendor:device + ROM/PROM hash）；换卡/换 ROM 即拒绝。
6. NVIDIA 固件不分发、不入 git；从钉 SHA-256 的官方输入本地再生；
   密钥/解密产物/载荷永不入库。
7. 写操作分级门控：M1 前零写；M3 进入前需用户显式确认；M4/M5 单独立项。
8. 先 mock 后真机：事务代码必须先在行为级模拟器全绿。
9. Secure Boot 关闭或自签证书入 db（优先后者）。
10. 许可：GPL-2.0-only。

## 回滚手册

### 场景 A：黑屏（链式启动失败/事务中断）
未动任何固件配置与 ESP 文件 → 强制重启 → 走原 Windows 引导项即恢复。
U 盘 BOOTX64.EFI 不在启动链上时机器根本不会进我们的代码。

### 场景 B：STOP/异常（事务中途失败）
同会话**绝不重试**（固件可能仍在运行、DMA 缓冲可能仍被占用）→ 完全关机断电
（笔记本按厂商流程放空 EC，ASUS 为拔电按住电源 40 s）→ 拔 U 盘 → 正常
Windows 引导 → 检查设备管理器/事件日志驱动状态 → 记录证据后再考虑下一次。

### 场景 C：Windows 驱动不认卡（交棒后异常）
设备管理器看错误码；事件日志查 nvlddmkm/Xid。解锁效果是易失覆盖（断电即失），
一次冷启动即回到出厂状态。若仍异常：再次完全断电；最后手段 DDU 重装驱动
（本工具不写任何持久存储，驱动状态异常只可能来自交棒时序，不是永久损坏）。

## 构建

```
cargo build --release   # 宿主侧 crate（assets/sim）——loader 不在 default-members
cargo efi               # EFI 只读构建（M0/M1 诊断模式）
cargo efi-tx            # gated 事务构建（含写路径；仅显式需要时使用）
cargo xt                # 全部宿主测试（assets 19 + sim 44）
```

裸 `cargo build --workspace` 会以宿主三元组构建 loader 并报错——loader 只能
为 `x86_64-unknown-uefi` 构建，请用上面的别名。

## 社区贡献与参照源

本项目是社区工作的直接受益者与衍生者，完整致谢见
[docs/ada28e0-uefi-report.md](docs/ada28e0-uefi-report.md) 的同名章节。核心：

- [gpu-unlock](https://github.com/Zagitta/gpu-unlock)（GPL-2.0）——事务链移植母本
- [gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada)（GPL-2.0）——
  AD10x 四写链源头；28e1 报告（Windroid234）是本项目直接依据
- [cmp170hx](https://github.com/Consensus-Protocol/cmp170hx)——PLM 语义与验证方法论
- [cmp90hx-unlock](https://github.com/WildFlash1st/cmp90hx-unlock)——V67 链源头
- [AD103_win_unlocker](https://huggingface.co/TOTORONG/AD103_win_unlocker)（TOTORONG）——
  Windows EFI 载具工艺先例
- [open-gpu-kernel-modules](https://github.com/NVIDIA/open-gpu-kernel-modules)（MIT）——
  引导路径一手权威

许可：**GPL-2.0-only**（`LICENSE`）。NVIDIA 固件与 VBIOS 版权归 NVIDIA，按其
原许可条款从钉版公开包本地再生，不在本仓库分发。
