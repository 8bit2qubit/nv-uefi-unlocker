# attempt 14-A：Windows 交接 TDR（Run A：全解锁 + 无 reconnect）

## 结果
解锁本体闭环（HANDOFF `tu10x: plm=0xffffffff ss0=0x88888888 ss1=0x8`）。
Windows：启动 logo 正常（VBIOS 显示无恙）→ 驱动接管即死帧缓冲 → TDR 风暴
→ 驱动 ForcedReset 自救成功（系统可用）→ **FP16 实测无解锁收益**。

## 事件判读
- `LKD_0x141_Tdr:6_IMAGE_nvlddmkm.sys_Turing`：dxgkrnl TDR，第 6 类，
  责任模块 nvlddmkm；
- nvlddmkm 153：`Resetting TDR` + `ForcedReset TDR on GPUID:100`——**驱动
  主动循环复位**（每次复位=FEAT 易失状态被擦）；
- LiveKernelEvent 117（视频硬件错误）+ dwm/udwm 崩溃（0xc00001ad，纯受害者）；
- WATCHDOG-20261027-1459.dmp 在用户机器上，可装 WinDbg 做 !analyze。

## 关键推断
1. **FP16 无收益不构成对 SS 写入的反证**：ForcedReset 在测量前已把易失
   FEAT 状态擦掉（同 FLR 重锁机制）。
2. reconnect 移除改变了失败形状（Code 43 → 起飞后 TDR）= 显示交接问题
   剥离了一层；剩余变量：**无 FLR 的交接状态** × **PLM open** × **SS=0x88888888**。
3. 参考对照的盲区重申：40HX/50HX 的 Windows 实证全部是无头矿卡，显示
   接管路径零覆盖；且它们进场时 handoff 已由 POST 推进（WPR2 up），
   我们进场 handoff=1 + 自点火 FWSEC。

## 隔离矩阵（每轮先断电冷启）
- **Run C（对照）**：拔掉 v67 → replay 模式（无 PLM/SS 写，仍无 FLR）。
  Windows 稳 → 凶手 ∈ {PLM open, SS 值}，继续 Run B；仍 TDR → 凶手 =
  无 FLR 的交接残留（SEC2/GSP/WPR2 半拆态），与解锁变量无关。
- **Run B**：v67 回位 + `\tu10x-no-ss.txt`（PLM open、无 SS 写）。
  Windows 稳 → **SS=0x88888888 是驱动杀手** → 测 FP16（稳定会话内）定
  TU106 出厂是否本就无钳制（开放 #10 终答）；仍 TDR → PLM open 本身与
  Turing Windows 驱动冲突 → EnableGpuFirmware 注册表 + 40HX Windows 工具
  细节考据。
- 可选：WinDbg（`winget install Microsoft.WinDbg`）分析 WATCHDOG dmp 的
  `!analyze -v`——TDR 超时的具体等待对象会直接落名。
