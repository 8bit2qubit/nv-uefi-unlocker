# M2 资产管线端到端验证报告（真输入，2026-10-25）

- 输入（assets/in/，gitignore 内，不入库）：
  - `NVIDIA-kernel-module-source-610.43.03.tar.xz` — sha256 `7e118923…` 校验 OK
  - `nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst` — sha256 `e17e7c92…` 校验 OK
- 命令与输出（逐级 hash 硬失败语义，任一不符即退出非零）：

```
$ cargo run -p assets -- extract …/NVIDIA-kernel-module-source-610.43.03.tar.xz assets/out
derived booter_load_ga102.bin: 61256 bytes, SHA-256 verified
derived gsp_boot_ga102.bin: 24576 bytes, SHA-256 verified
derived gsp_boot_desc_ga102.bin: 84 bytes, SHA-256 verified

$ cargo run -p assets -- fw …/nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst assets/out
gsp_ga10x.bin: 84277400 bytes, SHA-256 verified

$ cargo test -p assets
test result: ok. 10 passed   # bindata 往返/篡改拒绝/载荷钉 sha 复现/booter 校验/夹具
```

- 产出 hash 与社区钉死值（gpu-unlock artifacts manifest）逐位一致：
  - booter_load_ga102 `056cbc06…` ✓；gsp_boot_ga102 `3df651ac…` ✓；
    gsp_boot_desc_ga102 `cc5a3fc1…` ✓；gsp_ga10x `57237362…` ✓
  - V67 载荷生成器另在单测中字节级复现钉死值 `2a0aa1f8…` ✓

## 意义

任务书 M2 验收判据一"资产逐级 hash 绿"首次以**真输入**成立：Rust 管线
（archive.rs tar.xz/tar.zst 单成员抽取 → bindata.rs C 数组+raw-DEFLATE+gzip
CRC32/ISIZE 尾 → booter.rs bundle 组装+全部结构校验 → manifest 钉 hash）与
GPL 参考实现行为等价、产物逐字节一致（洁净移植达成）。

## 坑位记录

1. NVIDIA 源包根目录前缀 `NVIDIA-kernel-module-source-610.43.03/`（成员路径需带前缀）；
2. nvidia-utils 为 x86_64_v3 架构包（CachyOS 归档），zst 解压走 zstd crate 无碍；
3. 夹具测试三次修正（PATCH_META 12B/ucode 非零先行/DMEM 段不得越出镜像）——
   结构校验顺序与区间数学以 python 参考为准，夹具错说明校验器对。

## M2 剩余

- vbios.rs：BIT→ucode 表→FWSEC 描述符 V1/V2/V3（NVOC 侧 8353da3 经验移植）
- ELF 段抽取：gsp_ga10x.bin 为 ELF（.fwimage/.fwsignature_ga10x/_ad10x）
- TU106 资产钉 hash（从钉版 Turing 驱动源自抽——任务书 §9.1，逐芯片几何）
- sim/：test_unlock_adapter.c 行为级假 MMIO 移植 + 62 场景故障注入矩阵

## TU10x 资产闭环（2026-10-27 增补，真输入全绿）

上方"M2 剩余"前三条随 Turing 软解锁谱系搜证（docs/reading-notes.md）一并
关闭——对同一批钉版输入（release/inputs/，hash 前缀 7e118923/e17e7c92）：

```
$ cargo run -p assets -- extract-tu10x …/NVIDIA-kernel-module-source-610.43.03.tar.xz release/assets-tu10x
TU102 derived booter_load_tu10x.bin: 59224 bytes, SHA-256 verified   # 51507de6…（=nlzy artifacts json 同值）
TU102 derived gsp_boot_tu10x.bin: 4096 bytes, SHA-256 verified       # 8765694c…
TU102 derived gsp_boot_desc_tu10x.bin: 76 bytes, SHA-256 verified    # 9e7cca2c…（RM_RISCV_UCODE_DESC v4）

$ cargo run -p assets -- fw …/nvidia-utils-610.43.03-1-x86_64_v3.pkg.tar.zst release/assets-tu10x
gsp_tu10x.bin: 29352832 bytes, SHA-256 verified                      # 73065619…
  sections: .fwimage 0x1bfadc8 · .fwsignature_tu10x/_tu11x/_ga100 各 0x1000

$ cargo run -p assets -- gsp-sections release/assets-tu10x/gsp_tu10x.bin release/assets-tu10x
fwimage_tu10x.bin: 29339080 bytes · fwsignature_tu10x.bin: 4096 bytes

$ cargo run -p assets -- generate-tu10x release/assets-tu10x/v67-tu10x-four-plm.bin
wrote … (64000 bytes, SHA-256 verified)                              # b0ec9029…（=nlzy 钉版同值）
```

loader 期望的 tu10x 六资产（TX-ASSET-DIAG 清单）**全部产出、逐级 hash 绿**。
TU102 V67 载荷生成器为 nlzy generate_v67.py 的 GPL-2.0-only 行为级移植
（FEAT_PLM=0xffffffff + WPR2_HI=0 两写链 + ACR mutex-3 释放令牌，fill
0x4a7，guard 0x6140），单测字节级复现钉死值——我们的管线与 nlzy/cmp50hx
两条独立实现在派生资产上**逐位相同**，交叉验证成立。

## Turing tx 链启用缺口（M3 门控轮的工程清单，本报告仅记录不动手）

**→ 2026-10-27 全部关闭**（用户显式开闸，宪法 §8 以 sim 全绿先行）：

1. ~~main.rs:146 对 family=='tu10x' 的 run_transaction 前置拒绝~~ → 已解门；
2. ~~fwsec 段 Turing BOOT_WITH_LOADER 变体 + BSI SCRATCH_15 handoff 等待~~ →
   `fwsec_ignite_tu10x`（cmp50hx v70 配方：通用 BL 在 GSP IMEM 顶 tag 0xfd、
   BL DESC@DMEM0 由 BL 从主机缓冲 DMA 安全段进安全 IMEM、TRANSCFG0/4=0x15、
   poll WPR2；SCRATCH_15 在点火前后各留一行日志）；**V2 描述符解析字段偏移
   已按 50HX v68 实测修正**（旧解析把 VirtualEntry/IMEMPhys/SecBase 当成了
   iface/imemLoad/dmemLoad——真实布局 iface@16/imemLoad@24/SecBase@32/
   SecSize@36/dmemLoad@48/dmemOff@52）；
3. ~~booter_fire_direct 接线~~ → 自足化（内联 B-reset+BCR=Falcon+RM=chipId0
   +FBIF unblock），BOOTVEC=0、mailbox0/1=meta 64 位原址（v61：Turing 上
   <4GB meta 读会被锁，exit 0x91）；`verify_execution_tu10x`：replay 判据
   =mbox0==0+HALT+WPR2 up+**PLM 必须保持锁定**，payload 判据=FEAT_PLM 全开
   （TU102 载荷写的是 FEAT_PLM+WPR2_HI，GA10x 的 marker/WPRCFG 三证不存在）；
4. ~~WprMeta TU102 几何 + sim Turing 场景~~ → sim 增 replay 模式
   （`Transaction::new_replay` + BootMode::Halt 行为 + t20/t21/t22 场景，
   47/47 绿）。

**新增资产**：`sec2_bl_tu10x.bin`（768B，g_bindata_ksec2GetBinArchiveBlUcode
_TU102 → UCODE_IMAGE，sha256 `f21f1cfb…` —— 与 cmp50hx blobs/ 和 nlzy
artifacts json 的 sec2_bl_tu102 **三方逐位相同**）。loader 期望的 tu10x
资产从六件变七件。
