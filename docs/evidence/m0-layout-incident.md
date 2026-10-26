# M0 事件报告：PciIo raw 结构错位一槽 → 意外调用 Io.Write（险情，未遂）

- 日期：2026-10-25
- 触发：4060 Laptop（AD107）实机首测——33 个 PciIo 句柄零设备行；bootmgfw 找到但 splice 拒绝
- 定位过程：加状态日志后 QEMU 复现 → `Pci.Read` 恒 `UNSUPPORTED`、`GetLocation` 恒 SUCCESS+全零 →
  拉 EDK2 `MdePkg/Include/Protocol/PciIo.h` 逐字段对照

## 根因

EDK2 的 `EFI_PCI_IO_PROTOCOL` 开头是 **两个** poll 字段（`PollMem`、`PollIo`）。
手写 raw 结构只定义了一个 `_poll`，其后所有字段左移一槽（8 字节）：

| 我的字段（槽位） | 实际落到的函数 |
|---|---|
| `pci.read`（5） | **`Io.Write`（5）** ← 意外写原语 |
| `get_location`（13） | `Flush`（13）→ SUCCESS 且不写出参，解释全零 |

## 暴露面评估（如实记录）

- 意外调用形如 `Io.Write(This, Uint16, BarIndex=读偏移低字节, Offset=1, Count=<缓冲指针值>, Buffer=<栈垃圾>)`
  （win64 前四寄存器实参被六参被调函数重新解读，第 5/6 参取到垃圾）。
- QEMU（5 设备）与实机（33 设备）均返回 UNSUPPORTED/无效果——EDK2 对无 IO BAR 的设备在
  BarIndex 校验处提前拒绝。**没有任何写实际发生，但这是靠固件行为挡住的，不是设计保证。**
- 该 bug 版本曾在用户 4060 Laptop 上运行过一次（用户实测日志为证）。零写宪法（§1.7）
  在 M0 被意外突破过一次（未遂），特此立档。

## 修复与防再犯

1. 补 `_poll_io` 字段，结构与 EDK2 对齐；
2. **编译期布局钉死**（`const _: () = assert!(offset_of!(...) == ...)` ×4：`pci`=48、
   `get_location`=112、`rom_size`=144、总长=160），任何人改结构必炸编译；
3. 读回状态一律落日志（宪法 §1"读回是唯一成功判据"的投影：吞状态=盲飞，
   本事件在 QEMU 冒烟中被误判为"通过"就是因为负控只覆盖 not-found 路径+无状态日志）；
4. 负控盲区修正：VVFAT 上放置假 bootmgfw，使 found→splice→LoadImage 路径每次冒烟都被走到。

## 教训（进 README 安全宪法附注）

- **手写 raw FFI 结构必须以权威头文件逐字段对照**（EDK2 MdePkg > 记忆 > 教程），
  并用 offset_of 断言钉死；
- "看起来没崩"不是零写证明——只有每个调用的语义可审计才是；
- 同时修复的另外两个 bug：设备路径 END_ENTIRE 常量错为 `7F FF 00 00`（正确 `7F FF 04 00`，
  UEFI spec 端节点长度字段=4）；GetLocation 出参类型 `*mut u8`→`*mut usize`（UINTN），
  旧版若执行会踩栈（幸被 vendor 过滤短路从未执行）。

## 验证（QEMU q35，serial-fixed.log）

- 枚举出全部 5 个真实设备（8086:29c0/1234:1111/8086:2918/2922/2930，位置各异）✓
- GOP 探针：2 句柄 1280x800 ✓
- CHAIN：found → splice 成功 → LoadImage 被调用（假文件 → UNSUPPORTED，路径终点符合预期）✓
