# 必读清单进度（任务书 §4）

状态截至 2026-10-26：M0–M4 已闭环（v0.1.3 已发布；28e0 1.65×、28a1 1.7~1.9× 实测）。
里程碑门控的待读分组随对应里程碑完成而失效，保留作为执行记录；Turing 考据与
上游对齐两节仍为现行参考。

## 已读（本会话）

1. **`gpu-unlock/docs/How Ampere was unlocked.md`**（193 行，全文）——引导链六步时序
   （PCI→固件取载→devinit→FWSEC-FRTS/WPR2→SEC2 Booter→GSP-RM/RPC）；攻击点=booter 无界
   DMA 栈溢出→ROP；debug 微码用公开测试密钥（key index 37）加密→可反汇编挖 gadget；
   V67 载荷=单次任意写（MEMORY[addr]=value）；PLM 模型与 SS0/SS1 语义。
2. **`cmp170hx/docs/unlock/overview.md`**（211 行，全文）——四写 PLM 表（WPR_CFG 0xfffff0ff
   注意 bits[11:8] 留空、FBPA/WPR/FEAT=0xffffffff）；SS0=0x88888888/SS1=8；成功判据
   FEAT_READOUT_1(0x823818)=0（锁定卡读 0x016db6ed）；显存几何 CFG1/LMR 编码按 SKU；
   为什么成立三事实（总熔丝未烧/GA100 载 Turing 固件/debug 与 prod 同明文码）；
   实测收益表（FP32 31x、BF16 tensor 6.4→164-192 TF/s）。
3. **`cmp170hx/docs/unlock/privilege-level-masks.md`**（§1–§4 精读）——PLM 位编码
   [3:0]读层/[7:4]写层/[23:8]源使能；**0x000000FF 不是"开"**（源使能=0 全堵）；PLM 拒绝
   越界值（暴力枚举受硬件编码域约束）；锁定基线 0xFFFFFF8F 在全系 Ampere 一致；开 PLM 循环
   逐项最多 2 次尝试、每次点火前重臂 WPR2 lo/hi、**成功=读回精确相等**（booter 恒报 0x31 无意义）。

## 待读（按里程碑归组）

### M1 前（少量）
- TOTORONG HF 模型卡 + `docs/INSTALL.en.md`/`ANALYSIS.en.md`（EFI 载具交互范式细节）

### M2 前（全部）
- `gpu-unlock/kernel/gpu_unlock/unlock_610.c` 逐段
- `gpu-unlock/tests/test_unlock_adapter.c`（mock 模式→Rust 移植蓝本）
- `cmp170hx/docs/unlock/{how-it-works,falcon-and-booter,rop-chain}.md` 剩余部分
- `gpu-unlock-ada/docs/ada-port.md` + `rtx4090-register-report.md` + `tools/*.py`
- `open-gpu-kernel-modules`: `kgspBootGspRm` / `kgspPopulateWprMeta_TU102` / `kernel_gsp_fwsec.c`
- `cmp170hx/docs/unlock/register-reference.md`（冷值表对照）

### M3 前
- `gpu-unlock-bar1/docs/bar1-64g-report.md`（若涉及 BAR1）
- gpu-unlock packaging/boot（initramfs 时序，理解"驱动前"窗口的 Linux 侧实现）

## 已有外部知识衔接

- NVOC 侧 TU106 VBIOS FWSEC 解码（`docs/reverse-engineering/vbios/tu106-fwsec-ucode-decode.md`）
  已给出 V2 描述符几何（IMEM 0x9600/DMEM 0x410/adjust 0x11600）——M2 资产管线直接复用。

## Turing 引导路径考据（2026-10-26，钉版 610.43.03 源码一手，evidence-as-commits）

TU106 资产完成度推进时把 Turing 专属源码逐文件读完（`assets/in/` 钉版包内，
文件行号引用见下）。结论与 GA10x 社区链的**四处代际差异**，全部已落进代码：

1. **BooterLoad = BOOT_DIRECT，不是 BOOT_FROM_HS**（`kernel_gsp_tu102.c:79`
   `falconConfig.bBootFromHs = NV_FALSE`；GA102 为 TRUE）。TU102 BooterLoad
   BINDATA：SIG_PROD 仅 **16 字节**（非 RSA-384 表）、NUM_SIGS=1、ucode_id=**0xd**、
   patch@0x8700、image 59136=0xE700。内核侧照常 16B 签名补丁（kernel_gsp_booter.c
   `s_patchBooterUcodeSignature`，sigSize=total/numSigs，无 384 硬编码）。
2. **点火 = 寄存器 portal 直写，无 HS bootstrap、无 Falcon DMA 队列**
   （`kernel_gsp_falcon_tu102.c` s_prepareHsFalconDirect）：IMEM NS 段
   （fields[0:2]）落在 0、**secure 段（fields[5:6]）落在 ALIGN_UP(ns,256)**、
   DMEM（fields[2:3]）落 0；BOOTVEC=0；邮箱0/1=WprMeta 物理地址 lo/hi；
   start→HALT→**mailbox0==0 即成功**。这正是我们 AD107 上实证可用的 portal
   路径（tx_portal.rs）——Turing 内核自己就是用 portal 装载 booter 的。
3. **GspRmBoot 描述符 = RM_RISCV_UCODE_DESC v4（76 字节）**（rmRiscvUcode.h；
   TU102 实测 d0=4、bootloader 0..0x488、param 0x488+0x10、monitor/data/code
   全 0）。WprMeta 的 bootloader Code/Data/Manifest 三个字段=desc d12/d10/d8
   （TU102 上即 0/0/0），verbatim 拷贝即可。GA10x 是 v5（84B）。
4. **FWSEC-FRTS = BOOT_WITH_LOADER**（kernel_gsp_frts_tu102.c:411 分支）：
   经 **generic BL**（ksec2GetBinArchiveBlUcode TU102 BINDATA）装载——
   RM_FLCN_BL_DMEM_DESC 放 DMEM 0，BL 镜像放 IMEM 顶，BOOTVEC=blStartTag<<8
   （s_setupLoader）。与 VBIOS V2 描述符 BOOT_WITH_LOADER 的 M1 结论互证。
   WPR2 建立判据=NV_PFB_PRI_MMU_WPR2_ADDR_HI.VAL≠0（同我们的 0x1fa828）。

**不变量（可复用 GA10x 机制）**：GspFwWprMeta 256B 同构同 magic
（gsp_fw_wpr_meta.h:211 `0xdc3aae21371a60b3`、revision 1）；布局算法
kgspPopulateWprMeta_TU102 与 layout_610.c 同族（vgaWorkspaceOffset 从
mmuLockLo 取 min——我们 MMU_LOCK 冷值可读正好喂它）；kgspResetHw_TU102
止于 RESET_FALSE（§9.2 已知坑一致）。

**资产管线（tu10x_610 pins 已冻结复验）**：booter_load_tu10x.bin 59224B
（BTL1，内含 59136 图像+16B 签名）、gsp_boot_tu10x.bin 4096B（RISC-V
boot 段）、gsp_boot_desc_tu10x.bin 76B、fwimage_tu10x.bin 29339080B +
fwsignature_tu10x.bin 4096B（gsp_tu10x.bin ELF 抽取）。

**TU106 事务仍门控**：main.rs tx_phase 对 family=='tu10x' 在 run_transaction
前拒绝（"Turing transaction track not enabled yet"）——GA10x HS 链
（falcon_hs/FWSEC HS 路径）在 Turing 上是错误序列；启用前需把 tx.rs 的
fwsec 段换成 WITH_LOADER/generic-BL 变体 + BSI SCRATCH_15 handoff 等待
（kernel 用 SCRATCH_14 bit BOOT_STAGE_3_HANDOFF 判 reload 完成；冷态
scrubber 状态在 SCRATCH_15 bits31:29）。booter_fire_direct 已就位待接线。

## gpu-unlock-ada 上游更新对齐（2026-10-26，仓库更新至 d28c485）

上游 AD107 实验轨全面展开：28e0（4060 Mobile）/28a0/2882（4060 桌面）/
2803/2805/28e1 等全部在 experimental_ids 表内（各自 opt-in + 精确地址），
且 **28e1 有完整实测报告**（四写链在 AD107M 真卡 2.00× 提速，含稳定性
验证与真实负载 roofline）。链代码（unlock_610.c）自我们移植基线仅设备
表路由变化，无链条逻辑改动。

**交叉验证（我们 M1 独立测量 vs 28e1 报告）**：SS0/SS1 引导漂移 ✓、
WPR2_HI=0 入口判据 ✓、FBPA 停 0xffffff0f ✓、FEAT_READOUT 字段 2/5/7=2
（0x00410080 逐位吻合）✓、VBIOS 1MiB aperture ✓、四写值
（WPR_CFG=0xfffff0ff/WPR/FEAT=0xffffffff）✓、post-FLR selector 存活 ✓、
615.78.08 驱动版本解耦 ✓。AD107M 家族可行性从"我们 M1 独家推断"升级为
"社区实测证实"——用户的 28e0 与验证的 28e1 同芯片同厂（ASUS）。

**对齐修正（f26362ee）**：
1. payload 模式 FBPA 判据：0xffffff0f（三卡实测停止值）与 0xffffffff
   均接受——原判据会在真卡上假失败；
2. scrubber 改非致命尝试：unlock_610.c 原文 "GA102 has no scrubber"，
   社区全部验证卡（含 28e1）无 scrubber 步骤即完成链——我们尝试后无论
   结果都继续（verdict 留日志），链条是否需要跨过门控态由实机裁决；
3. EALREADY 改记 skip（成功类，da96e3f 语义）且不 FLR——卡已解锁时
   FLR 会抹掉活的解锁。
