# M0 QEMU+OVMF 冒烟报告（负控测试）

- 日期：2026-10-25
- 环境：QEMU 11.1.0 (winget) + 自带 edk2-x86_64-code.fd (edk2-stable202408)，q35/512MB，`-display none -serial file:`，VVFAT 目录 `sim/qemu/usbroot/` 作为启动盘
- 产物：`loader.efi`（debug 构建，uefi 0.41，commit 待填）→ `EFI/BOOT/BOOTX64.EFI`
- 原始日志：[serial.log](serial.log)

## 判据核对

| M0 验收项（QEMU 部分） | 结果 |
|---|---|
| OVMF 从启动盘进入我们的 app | ✅ `BdsDxe: starting Boot0001 ... QEMU HARDDISK` 后进入 loader |
| 版本横幅 + 阶段日志（TOTORONG 范式） | ✅ CONFIRM/ENUMERATE/CHAIN/FINAL 四阶段全输出 |
| 30s 超时路径 | ✅ `CONFIRM: 30s timeout, proceeding`（TCG 下墙钟偏慢但路径正确） |
| **负控：无 NVIDIA 设备→优雅继续** | ✅ 枚举 5 个 PciIo 句柄（q35 自带设备），无 10de 设备，无崩溃无挂起 |
| bootmgfw 不存在时的行为 | ✅ 正确报 `NOT_FOUND`，不重试不写任何东西 |
| app 返回后固件继续正常引导 | ✅ BdsDxe 继续 `Boot0000 UiApp` 进前端菜单 |
| **零 GPU 写审计** | ✅ 见下 |

## 零写审计（宪法 §1.7）

```
$ grep -rn "write" loader/src/ | grep -v "^.*//" | grep -iv "MemAccess\|write_bar\|_write:"
```

`pci.rs` 定义了 raw 结构中的 `MemAccess.write` / `CfgAccess.write` 函数指针槽位（结构体布局
必需），但全仓库**无任何调用点**；M0 代码路径只有 `config_read_u8/16/32` 与 `get_location`。
chainload 只读探测文件 + LoadImage/StartImage。U 盘/VVFAT 无任何写操作。

## 偏差记录（相对任务书 §5/§8）

1. 任务书预览的 API 形态 `boot::locate_handle_buffer` 对应 **uefi 0.41**（crates.io 最新）；
   uefi-services 已弃用合并回主 crate（`uefi::helpers::init()`）。加载器已用 0.41 + 自由函数 API。
2. uefi 0.41 **没有设备级 PciIo 协议封装**（`proto/pci` 是 RootBridgeIo）——按任务书预案降级：
   `loader/src/pci.rs` 自写 raw `EFI_PCI_IO_PROTOCOL`（GUID 4cf5b200-68b8-4ca5-9eec-b23e3f50029a）。
   M1 的 BAR0 访问将在此结构上补 `mem.read` 封装（仍是只读）。
3. `OpenProtocolAttributes::GetProtocol`（spec 的 EFI_OPEN_PROTOCOL_GET_PROTOCOL）用于枚举，
   不用 Exclusive——Exclusive 会触发驱动断连语义（这正是 M3 GOP 断连用的机制），枚举阶段必须避免。

## 待办（M0 实机部分，需用户在场）

- TU106 台式机 U 盘实启：Secure Boot 状态确认 → U 盘首启 → 枚举 10de:1f07 → 屏显 → 链回 bootmgfw 无缝进 Windows
- 实机日志抄录/照片归档后 M0 关账
