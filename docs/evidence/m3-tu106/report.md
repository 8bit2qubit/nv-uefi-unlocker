# M3 TU106 无载荷链闭环报告（2026-10-27，attempt 7）

日志：`attempt7-full-chain-complete.log`。判定行：**`FINAL: transaction
complete, all DMA released`** + `SELECTOR RESULT: transaction complete` +
链回 bootmgfw。任务书 M3 验收判据逐条对账：

| 判据 | 证据 |
|---|---|
| WPR2 建立 | `FWSEC-TU10X: *** WPR2 UP lo=0x1ffe000 hi=0x1ffee00 after 3ms ***`（= 8GiB-2MiB FRTS，与 40HX dmesg 逐位相同）；attempt 4/5/7 三次复现 |
| 完整读回记录 | AUTH-SCAN 46 寄存器差分 → layout → FWSEC 每步读回 → portload 几何 → BOOTER-DIRECT mbox/EXCI → POST-BOOT 四判据 → FLR 前后状态 → HANDOFF-OUT |
| 交棒前状态清零 | `POST-FLR: WPR2=0x1ffffe00/0x0`（冷态默认值模式），PLM 保持锁定，无残留半状态 |
| Windows 驱动正常接管 | 链回 bootmgfw ✓；Windows 侧健康检查由用户确认（Device Manager / nvidia-smi） |

## 链条各段的 Turing 专属事实（全部一手实测）

1. **FWSEC WITH_LOADER**（cmp50hx v70 配照移植）：通用 BL@GSP IMEM 顶
   （tag 0xfd，dst 0xfe00，本卡 HWCFG=256 块 vs 50HX 卡 255 块）+ BL
   DESC@DMEM0 + TRANSCFG0/4=0x15 → BL 从主机缓冲 DMA 安全段。3ms 建 WPR2。
2. **BOOT_DIRECT booter**：SEC2 直载（NS 256B@0 + SEC 33792B@0x100 + DMEM
   25088B），BOOTVEC=0，mailbox0/1 = meta 64 位地址（**输入载体语义**，
   停机后保留输入，无状态返回口）。HALT + EXCI=0。
3. **replay 判据**：EXCI==0 + WPR2 up + PLM 保持锁定（+FACTORY READOUT
   badf 负控）。GA10x 的 mbox0==0 与 marker 三证不适用于 Turing。
4. **Turing PL0 锁死清单**（Ada 全可读 → Turing 全锁，代际差异）：
   FB_SIZE_MIB、MMU_PLM、FUSE 簇、FEAT 簇 13/13——layout 需 SKU 回退、
   单签名 bundle 免 fuse、锁位检查跳过。
5. **flr 后 GPU 自举**：FLR 重启中 GPU 自行 provision WPR2
   （wpr2_hi=0x1bfe00，三次复现）——cmp50hx"POST 已拉起 WPR2"的机制实证。

## 修复序列（attempt 1-6，全链六道门）

fb SKU 回退 → MMU_PLM 检查跳过 → BCR VALID 不等（Turing 无核切换）→
单签名免 fuse → num_sigs==1 补丁路径 → BOOT_DIRECT mailbox 语义 +
flr selector 容忍。每道门一条 log 定位、一次修复、下火验证。

## 遗留观察（不影响闭环）

- BSI15/SCRATCH14 在 replay 后未推进（handoff=1 不变）——内核路径的推进
  发生在有驱动在场的 reload；冷 replay 无驱动，符合预期。
- GSP_CPUCTL 停机后 =0x10（HALT）：无驱动消费方时 booter 不留 GSP-RM
  常驻。
- 全部 DMA <4GB：本卡（消费 TU106）无碍；锁定矿卡的 <4GB 锁不适用。
