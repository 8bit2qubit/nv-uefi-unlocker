# M0 实机验收报告——AD107 笔记本（4060 Laptop，独显直连）

- 日期：2026-10-25
- 机器：ASUS G614JV 系笔记本（RTX 4060 Laptop = AD107M，10de:28e0），独显直连（MUX）模式
- 产物：loader.efi release（commit 7a03cf2 之后构建），U 盘 `EFI\BOOT\BOOTX64.EFI`
- 原始日志：[loader.log](loader.log)（**来自 U 盘根目录的日志文件本身**——双写模块的第一次实机产出）

## M0 验收判据核对（任务书 §6 M0，按本机范围）

| 判据 | 结果 |
|---|---|
| U 盘启动到 app | ✅ 回车确认（`Printable('\r')`）进入 |
| 屏显枚举结果 | ✅ 33 设备全列出 |
| 无缝链回 Windows | ✅ `LoadImage bootmgfw` 后日志按设计终止于 finish()，控制权交接成功 |
| 零 GPU 写 | ✅ 全程只读（配置空间读 + 文件系统探测）；唯一写=U 盘自身 `\loader.log` |

**M0 在本机正式通过。** TU106 台式机（任务书主力实验机）仍待验证。

## 关键观测（M1/M3 直接可用的数据点）

1. **GPU 身份**：`10de:28e0 @ 0000:01:00.0`（AD107M 实测确认），音频函数 `10de:22be @ 01:00.1`。
2. **BAR 布局**：BAR0 32-bit MMIO `0x83000000`（16MB，M1 采样窗口）；BAR1 64-bit `0x6000000000`（VRAM，above-4G）；BAR3 64-bit `0x6200000000`；BAR5 IO `0x5000`。
3. **FLR 能力**：GPU 与音频函数 DEVCAP bit15 均置位（`FLR yes`）——M3 事务链的 FLR 步骤在本机可行。音频函数与 GPU 同组，FLR 时注意作用域（按 function）。
4. **链路**：LINKCAP `0x00456884` → 最大速率 Gen4 ×8（AD107 移动端典型配置）。
5. **ReBAR**：扩展能力存在（@0x0bb0）但 `0 resizable BAR(s)`——BIOS 管理的固定 BAR 布局，不可运行时调整。M5 BAR1 议题在本机无 ReBAR 通道。
6. **扩展能力清单**：0x0001(AER)@0x420、0x0002(VC)@0x100、0x0004(功率预算)@0x128、0x000b(**NVIDIA 厂商自定义**)@0x600、0x0018(L1 PM 子状态)@0x250、0x0019(Secondary PCIe)@0x900、0x001e@0x258、0x0025/26/27(物理层相关)@0xe00/0xc1c/0xd00。
7. **GOP**：单句柄 `2560x1600 Bgr` = 内屏面板经独显直连路径（nvgop-ad1xx 驱动，drivers 列表 1CD 条目佐证）。句柄未与 PciIo 直接关联（无 `[NVIDIA GOP]` 标注）——GOP 生产句柄与 GPU 句柄不同对象；M3 断连时需按 GOP 句柄本身操作，不能想当然用 PCI 句柄。iGPU GOP 本次未出现（直连模式下无输出绑定）。
8. **日志终止位置**：最后两行=finish() 关闭日志 + LoadImage 成功，StartImage 后控制权在 Windows——这正是设计行为，不是丢失。

## 与 QEMU 冒烟的互补

QEMU 负控（无 NVIDIA→优雅跳过）+ 本机正控（有 NVIDIA→完整描述+链回）+ 事件档案
（m0-layout-incident.md）三者构成 M0 的完整证据链。
