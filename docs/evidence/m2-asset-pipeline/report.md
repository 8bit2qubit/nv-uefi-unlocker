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
