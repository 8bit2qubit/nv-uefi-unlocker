# 寄存器冷值表（M1 采样产出，按族分节）

> 每次实机采样后把 `\loader.log` 的 SAMPLE 段誊入对应节，保留原始日志于
> `docs/evidence/m1-<machine>/`。冷值=驱动加载前（UEFI 阶段）读回值。
> badf 桶：PRIV_VIOLATION=0xBADF5040 / UNDECODED=0xBADF1100 / FLOORSWEPT=0xBADF20xx。

## TU106（RTX 2070 台式，10de:1f07）——已采样（2026-10-26）

| 寄存器 | 冷值 | 解读 |
|---|---|---|
| PMC_BOOT_0 | 0x166000a1 | TU106 ✓ |
| **FEAT 簇 13/13 badf5040** | PRIV_VIOLATION | **簇存在但 Turing 在 PL0 全锁定**（开放问题 #1 关闭） |
| FEAT_READOUT_1 | badf5040 | PL0 无法判定钳制态 |
| GSP_CPUCTL 0x110100 | **0x10 可读** | RISC-V 核 CPUCTL 界面 PL0 可读、HALT 位同义（开放问题 #4 部分关闭；BCR 0x668 切换语义待事务轮核实） |
| SEC2_CPUCTL | **0x10 可读** | 与 AD107（badf5620 门控）代际差异 |
| BSI handoff (SCRATCH_15 0x1180fc) | 0x202001bf（bits31:29=1） | 与 AD107 一致：pre-driver 卡在 state 1 |
| WPR2_LO/HI 0x1fa824/28 | 0x1ffffe00 / 0x0 | 与 AD107 冷值完全相同——GFW 后空窗口默认态（非异常）；hi=0 守卫放行 |
| MMU_LOCK_LO/HI 0x1fa82c/30 | 0x00100000 / 0x01effff0 | 可读，WPR 布局计算输入（VBIOS 锁定区） |
| MMU_PLM / FUSE 簇 | badf5040 | 锁定 |
| FBPA_PLM 0x9a0148 | 0xffffffcf | 可读，值异于 AD107 |
| AD1xx 名义寄存器 CYA/CFG/BAR1_CAP | 0x8 / 0x2 / 0x1c00 | Turing 上地址语义未核实，仅记录（跨族地址采样噪声） |
| NV_PROM 64K | crc32 0x552e185c | 本卡身份 |
| post-FLR GFW_BOOT | 0x101 卡住 | 与 AD107 一致（跨代 FLR 行为）

## GA106（RTX 3060，10de:2504）——M4 候选机

不在本轮范围（M4 单独立项）。

## AD107M（RTX 4060 Laptop，10de:28e0）——已采样（2026-10-25，见 evidence/m1-ad107-laptop/）

| 寄存器 | 冷值 | 解读 |
|---|---|---|
| PMC_BOOT_0 | 0x197000a1 | impl 0x197 = AD107 ✓ |
| FB_SIZE_MIB | 0x1ffc（8188MiB） | 8GB 布局 |
| GFW_PLM / SCRATCH | 0x00008b8f / 0x000003ff | GFW 已跑完（scratch 就绪态） |
| GSP_CPUCTL 0x110100 | 0x10（HALTED） | GSP 冷态停机 ✓ 驱动未点火 |
| MMU_PLM | 0x0045cb8f | 写锁定型 PLM |
| WPR2_LO/HI | **0x1ffffe00 / 0x00000000** | GFW 后的空窗口默认态；hi=0→M3 冷启动守卫放行（任务书"恒 0"应修正为"hi 恒 0"） |
| SEC2_CPUCTL 0x840100 | **0xbadf5620** | **新发现的 badf 变体（0x56xx 桶）**，语义待 M2 对照 cmp170hx register-reference |
| FUSE_OPT_FEAT_OVR_DIS 0x8203f0 | **0x0** | **总开关未烧——解锁域在本芯片存在**（与 CMP 170HX 同前提） |
| FUSE_VER / DECODE | 0x1 / 0x1 | 签名槽位信息（M2 资产核对） |
| FEAT 0x823800（ECC PLM） | 0xffffff8f | 与 GA10x 锁定基线一致 |
| **FEAT 0x823804（FEAT_OVR_PLM）** | **0xffffff8f** | **锁定基线，与全系 Ampere 相同——钳制门就在这里** |
| FEAT 0x823808/80c/810/814 | 0x182 / 0x00888888 / 0x00aaaaaa / 0x233 | 其他 feature 类别 fuse 派生值（M2 对照解码） |
| **FEAT 0x823818（FEAT_READOUT_1）** | **0x00410080 ≠ 0（两次引导稳定）** | **算力钳制存在！** 九个 3 位字段中 #2/#5/#7 = 2（约 1/4 速率），其余 0 |
| **FEAT 0x82381c（SS0）** | boot1: 0x17142071 / boot2: 0x05142071 | ≠GA102 默认 0x00000001——**且引导间有漂移**（最高 nibble 变化；n=2，M2 模拟器按"SS0 冷值非确定"建模） |
| **FEAT 0x823820（SS1）** | boot1: 0x00000004 / boot2: 0x00000005 | 同上，引导间漂移 |
| FEAT 0x823824/828/82c | 0 / 1 / 0xa | 记录 |
| FEAT 0x823b00（行重映射 PLM） | 0x00004b8f | 与 WPR 重锁型 PLM 同族 |
| AD1xx CYA / CFG | 0x2d / 0x80000007 | CFG≠4090 的 0x8000000a——**逐 SKU 差异实证** |
| AD1xx XVE_PLM | 0xffffff8f | 锁定基线 |
| AD1xx BAR1_CAP_MIRROR | 0x0003fc00 | 记录 |
| **Ada 链 WPR_PLM 0x1fa7c4** | **0x0004cb8f** | **与 cmp170hx 已知模式逐位吻合**：WPR/WPR_CFG 在原厂驱动加载后重锁值——冷态即此值，四写链目标在位 |
| **Ada 链 WPRCFG_PLM 0x1fa7cc** | **0x0004cb8f** | 同上 |
| **Ada 链 FBPA_PLM 0x9a0148** | **0xffffff0f** | 与任务书 §7.4 Ada 链"观察项，卡 0xffffff0f 记非致命"记录完全一致（社区在真卡上见过的同一个值） |
| NV_PROM 64KiB | crc32 0xd4a7daa2，magic 0xeb7eaa55 | 本卡身份钉死（0xaa55 ROM 签名 ✓，三引导一致） |

**SS0/SS1 引导间漂移（n=3）**：SS0 = 0x17142071 / 0x05142071 / 0x04140071（三个不同值，
高位字节漂移+低位偶变），SS1 = 0x4/0x5/0x4；FEAT_READOUT_1 恒 0x00410080——
漂移只在覆盖寄存器冷值，不在生效读出。sim 已按此建模。

**M1 判据核对（本机）**：采样落盘 ✓；badf 分桶（发现新桶 0x56xx，分类器已扩展）✓；
FEAT 簇 13/13 可读（存在性=是，PL0 可读）✓。

**两个采样器 bug 同轮修复**：FEAT 扫描 Range<u32> 按字节迭代（36 次非对齐读被
计为 ok——教训：迭代语义与读取宽度要对齐建模）；0xbadf5620 无桶（加 BADF_VARIANT
兜底桶 + 读错误独立计数）。

## AD107M 第二张卡（RTX 4050 Laptop，10de:28a1）——首次实机（2026-10-26，混合输出模式）

同一 G614JV，卡换成 4050。事务在 layout 读数处失败（VGA 毒值把整条 layout 拒掉），
FLR+链回 bootmgfw 正常。

| 寄存器 | 值 | 解读 |
|---|---|---|
| FB_SIZE_MIB | 可读（6GB 量级） | 显存已训练，不是 D3cold |
| **VGA_WORKSPACE 0x625f04** | **0xbadf1301** | **新毒值（0x13xx 桶）**——该寄存器门控。只有 bit3 参与布局判决，bit3=0 → 与健康卡走同一条"FB 顶 1MiB 保留"路径 |
| SEC2_CPUCTL 0x840100 | 0xbadf5620 | 与 28e0 相同的冷态域门控（复位后可读，scrubber 通过） |
| GOP 句柄数 | **2** | iGPU 也在 console 里 |

**同机同模式的对照**（2026-10-26，28e0 混合关机、只有 dGPU 的 1 个 GOP、全程跑完：
VGA 读回正常 → layout 走 bit3=0 → vga=FB 顶；FWSEC WPR2 验证、booter 点火、四写、
选择器穿 FLR）。因此 0xbadf1301 的关联量是"**dGPU 是否是 console owner**"而不是"混合模式"：
dGPU 主显时该寄存器正常，iGPU 占 console 时被门控。第三种状态是 iGPU-only 关机——
dGPU 冷、无 GOP、BAR0 全 0xffffffff（claim 前采样即此现象，claim 后仍全 1 即 D3cold）。
混合模式下的第四种：休眠/唤醒后 dGPU 停在 D3（resume 不走 POST、不跑 devinit），
同样读到 0xbadf1301；独显直连则休眠与重启均正常。

**修复**：`layout_from_registers` 的五个输入改 raw 读，FB/MMU 锁仍严格拒毒，
仅 VGA workspace 容毒并回退（log 打出原值与 MUX 提示）。教训沿用 §3.3：毒值先分桶再
决定"可用/不可用"，全 BADF 一刀切成错误会丢掉可用信息。

## 已知对照值（社区来源，供采样比对）

| 寄存器 | GA100/GA102 锁定冷值 | 来源 |
|---|---|---|
| FEAT_OVR_PLM 0x823804 | 0xffffff8f | cmp170hx（全系 Ampere 一致） |
| FEAT_OVR_ECC_PLM 0x823800 | 0xffffff8f | 同上 |
| 0x823B00 行重映射 PLM | 0xffffff8f | 同上 |
| FEAT_READOUT_1 0x823818 | 0x016db6ed（170HX 锁定） | cmp170hx |
| WPR2 lo/hi | 0/0（FWSEC-FRTS 前恒 0） | 任务书 §3.2 |
| FUSE_OPT_FEAT_OVR_DIS 0x8203f0 | 0x0=总开关未烧（存在前提） | cmp170hx |
