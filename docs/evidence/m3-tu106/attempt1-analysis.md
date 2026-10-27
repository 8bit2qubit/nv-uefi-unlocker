# M3 TU106 首火分析（attempt 1，2026-10-27）

日志：`attempt1-layout-eio.log`。结果：**在 layout 阶段干净失败**——未触碰任何
固件点火路径，FLR 收尾 + 正常链回 Windows（HANDOFF-OUT 完整）。

## 失败点

```
layout inputs: fb_mib=0xbadf5040 ... image=0x1bfadc8 boot=0x1000
TX: transaction failed: EIO
```

`FB_SIZE_MIB`（0x1183a4）在 Turing 上 PL0 被 PLM 锁死（M1 两次采样一致：
badf5040；Ada 上该寄存器可读——代际差异又一项）。layout 拿不到显存大小即 EIO。

**修复（已入库）**：`layout_from_registers` 增 family 参数；tu10x 且 fb 不可读
时按 SKU 几何回退（10de:1f07 = 8192 MiB，cmp50hx detect_fb_size 同款思路），
其他 TU106 SKU 显式拒绝并要求扩表。

## 顺带的一手发现

1. **GPU 自 FWSEC 化**：emergency FLR 后的 GPU 自重启里 `wpr2_hi=0x1bfe00`
   ——本板 GFW/BSI 自举链在 FLR 重启中**自行 provision 了 WPR2**（无任何主机
   FWSEC 点火）。这正是 cmp50hx 观察到的"POST 已把 WPR2 拉起"现象的机制：
   在它们的板上 POST 就拉起，在本板上 FLR 重启拉起。含义：
   - tu10x 链已按 cmp50hx 主线补"WPR2-up 入口 → 跳过 FWSEC 点火"路径；
   - 若下次入口 hi≠0，链会直接走到 booter（更短的路径，同样有效）。
2. **scrubber 外来二进制不可点**：AD10x scrubber ucode（8264B）在 TU106
   SEC2 上 EIO（预期内——不是本族的 ucode）。tu10x 链已改为跳过该步并留日志
   （cmp50hx 链本就无 scrubber 步）。
3. **AUTH-SCAN**：全寄存器可读、双引擎对称。GSP 侧 IMEMC=0x91009600 /
   IMEMT=0x95 / TRACEPC=0x55f 是 POST 遗留的 IMEM 窗口状态（falcon_reset
   会清）；SEC2 侧全零 + CPUCTL=0x10，干净冷态。
4. ** scrubber 时 BSI handoff=1（DONE=3）**：与 M1 冷态一致；ADR107M 上该
   状态被内核 scrubber 推进，Turing 侧 cmp50hx 链证明不推进也能走通全链。

## 下次点火预期路径

入口（假设冷态 hi=0）：claim → AUTH-SCAN → 跳 scrubber → layout（SKU 回退
8192MiB，frts = 8GiB-2MiB = 0x1FFFE00000）→ **FWSEC-TU10X**（BL 装载 + TRANSCFG
0x15 + poll WPR2）→ BOOTER-DIRECT（BOOTVEC=0，mailbox=meta 64 位）→ POST-BOOT
（mbox0==0 + HALT + WPR2 up + PLM 保持锁定 + FACTORY READOUT 负控）→ selector
probe（预期 readback=fuse 值不变）→ FLR → HANDOFF-OUT。

若入口 hi≠0（POST/FLR 遗留）：跳过 FWSEC 直达 booter——这是 cmp50hx 板上
每次都走的主线路径。

---

## attempt 2（2026-10-27，log: attempt2-mmu-plm-eio.log）

fb 回退生效后，下一道门=**MMU_PLM（0x1fa7c8）在 Turing 上同样 PL0 锁死**
（badf5040）。该寄存器只喂 bit0 锁位 sanity check——真正的布局输入 lo/hi
锁窗口在 TU106 上可读，且与 AD107M 的 M4 实证值**逐位相同**
（0x00100000/0x01effff0）。

**修复（已入库）**：plm 不可读 + tu10x → 跳过锁位检查（留日志），lo/hi 继续
作为布局输入。lo/hi 不可读仍硬失败（Ada 语义不变）。

**下一火的预期 layout 输出**（8192MiB SKU 回退 + VGA bit3=0 路径，
mmu 锁窗口因 mmu_top≠FB top 走"lock ignored"分支——与 AD107M M4 同款）：
`frts = 0x1FFE00000`（= 8GiB-2MiB，与 40HX dmesg WPR lo=0x01ffe000 逐位吻合）
→ 随后进入 FWSEC-TU10X 点火段。

---

## attempt 3（2026-10-27，log: attempt3-bcr-valid-timeout.log）

**layout 全绿**：fb=0x200000000、frts=0x1ffe00000（= 8GiB-2MiB，与 40HX dmesg
WPR lo=0x01ffe000 逐位吻合）、wpr=[0x1f8900000,0x1fff00000]。FWSEC-TU10X 段
启动并全部走通到点火前：

- V2 描述符（本板 PROM）：**NS 0..0x300 + SEC 0x9300@0x300、dmem 0x410
  @img 0x9600、iface@0xdc、stored=0x9a10**——与 NVOC 实测自洽（iface 0xdc
  非 40HX 的 0xe0，blob 版本差异，walker 自适应找到 mapper cmd@0x3d0 并
  成功写入 frts）；
- 失败点：`falcon_reset` 的 BCR VALID 等待超时（0x111668 bit0 永不置位）。
  **cmp50hx v66 明言"Turing 无显式 core switch……BCR 读回恒 0x0 属正常"**——
  我们的 falcon_reset 按 AD107 语义（GA102 kflcnResetIntoFalcon：VALID=1 才
  算完成）等了一个永远不会来的位。

**修复（已入库）**：`falcon_reset` 增 `switch_to_falcon` 参数——Turing 路径
（fwsec_ignite_tu10x / booter_fire_direct）写 BCR=0 + 16 次读回 + 10ms，
不等 VALID；Ada/GA10x 路径（4 处）保持原语义。

**下一火预期**：reset 过后 → RM=chipId → TRANSCFG0/4=0x15 → BL DESC@DMEM0
→ BL@IMEM 顶（读回双验证）→ BOOTVEC=0xfd00 → STARTCPU → **WPR2 poll 5s**
（第一次真正的 FWSEC 执行判定）。

**风险清单**：FWSEC 缓冲落在 dev=0x3a05b000（<4GB）——50HX v2.62 把 FWSEC
缓冲放到 >4GB（v2.63：锁定矿卡 <4GB 读被锁）。消费卡大概率无碍；若 WPR2
poll 超时，此为头号嫌疑。

---

## attempt 4（2026-10-27，log: attempt4-fwsec-up-fuse-eio.log）——**FWSEC 点火成功里程碑**

```
FWSEC-TU10X: BL at IMEM dst=0xfe00, BOOTVEC=0xfd00
FWSEC-TU10X: STARTCPU, polling WPR2 up (5s budget)...
FWSEC-TU10X: *** WPR2 UP lo=0x1ffe000 hi=0x1ffee00 after 3ms ***
FWSEC-TU10X: WPR2 verified (FRTS at 0x1ffe00000)
TX: fwsec ok (WPR2 verified)
```

**本项目第一次驱动认证固件在 Turing GSP 上执行并拿到读回证据。** WPR2
3ms 建立，lo/hi 与 40HX 真机 dmesg（WPR=01ffee00:01ffe000）**逐位相同**。
BL dst=0xfe00≠0xfd00 警告无碍：IMEM 按 tag 寻址，va 0xfd00 正确映射物理块
（FWSEC 跑起来即证明；50HX 卡读出 255 blocks、本卡 256 blocks，HWCFG 差异）。

**新失败点**：五笔 DMA 分配后、booter_patch_stock 前的
`FUSE_VER + (ucode_id-1)*4` 读（0x824170）→ Turing FUSE 簇 PL0 锁死
（badf5040，M1 已示警）。而 Turing BOOT_DIRECT bundle 只带一张 SIG_PROD
签名（1×16B），fuse 槽位选择无从谈起。

**修复（已入库）**：`num_sigs==1` → 跳过 fuse 读、fuse_ver=0（补丁索引 0
=唯一签名）；多签名 bundle 仍走 fuse 读（Ada 语义不变）。

**下一火预期**：patch SIG_PROD@0x8700 → radix(29MB) → WprMeta 发布 →
bridge BME → **BOOTER-DIRECT**（SEC2 直载，BOOTVEC=0，mailbox=meta 64 位）
→ POST-BOOT 判据（mbox0==0 + HALT + WPR2 up + PLM 保持锁定 + FACTORY
READOUT=badf 负控）。

**在挂嫌疑**：全部 DMA 落 <4GB（0x3a0xxxxx）——v61 锁定矿卡上 <4GB 读被
锁（exit 0x91）；消费卡大概率无碍。若 booter 回 0x89/0x91，改分配策略 >4GB。

---

## attempt 5（2026-10-27，log: attempt5-single-sig-erange.log）

fuse 跳过生效（fuse_ver=0）。新失败点：`booter_patch_stock` 的
`num_sigs<2 → ERange` 守卫——那是 GA10x/Ada 双签名 fuse 槽位表的约束，
Turing BOOT_DIRECT bundle 只有一张 SIG_PROD（1×16B），整包被拒。

**修复（已入库）**：num_sigs==1 → 直接补丁 signatures[0..sig_size] 于
patch_loc，跳过槽位索引；多签名路径逐字未动。

**FWSEC 里程碑复读**：attempt 4/5 两次点火，WPR2 均 3ms 建立、值逐位稳定
（lo=0x1ffe000 hi=0x1ffee00）——FWSEC-TU10X 已是可复现路径。

**下一火预期**：patch@0x8700 → radix 29MB → WprMeta 发布（含 magic 读回）
→ bridge BME → BOOTER-DIRECT（SEC2 falcON reset + NS/SEC/DMEM portload +
BOOTVEC=0 + mailbox=meta 64 位 + STARTCPU）→ POST-BOOT 四判据 + FACTORY
READOUT。DMA 全 <4GB 嫌疑仍在挂（booter 若回 0x89/0x91 即换 >4GB 策略）。

---

## attempt 6（2026-10-27，log: attempt6-booter-halted-mailbox-input.log）——**BOOTER 执行里程碑**

单签名补丁生效 → radix 7163 页/16 表（29MB RM 镜像）→ WprMeta 发布 →
bridge BME（全路径已是 bus master，Intel 桥无需修）→ **BOOTER-DIRECT**：
SEC2 reset → NS 256B@0 + SEC 33792B@0x100 + DMEM 25088B portload →
STARTCPU → **HALT**。

`mailbox0 = 0x3a0de000` = 我们传入的 meta 设备地址。**语义发现**：TU102
BOOT_DIRECT 约定 mailbox0/1 = meta 输入载体（内核传 64 位地址进栈），
停机后**保留输入**——不存在 GA10x 的"mbox0=状态码"返回口（50HX 的判定
也从不用 mbox0，直接读 PLM/SS）。旧判据误报 EIO。

**修复（已入库，三处）**：
1. booter_fire_direct：HALT 后记录 mbox0/mbox1/EXCI，不再拿 mbox0 当状态；
2. verify_execution_tu10x replay 判据 = **EXCI==0 + WPR2 up + PLM 锁定**
   （硬证据），另记 GSP_CPUCTL/SCRATCH14/BSI15 启动进展；
3. flr_phase 的 pre/post-FLR selector 读：Turing 上 badf 锁定为预期态，
   unwrap_or(0)（replay 不比较）。

**下一火预期**：若 EXCI==0 且 WPR2 保持 up → replay 判据通过 → 直达 FLR →
HANDOFF-OUT → **M3 无载荷链全闭环**（判定行 = "replay verdict ok"）。
若 EXCI≠0：booter 异常，四件套日志定位。

---

## attempt 8（2026-10-27，log: attempt8-payload-meta-rejected.log）——payload 首发空枪

payload 装载正常（64000B，签名 DMA 16 页，mailbox0=sig 地址=输入载体）。
booter 干净 HALT、EXCI=0、**PLM 未动**——ROP 未起爆。

**根因（meta 校验拒绝）**：TU102 booter 会校验 WprMeta 的 heap 几何字段，
不符则拒收整份 meta、跳过签名 DMA、干净退出（50HX v2.63 原话："heap 1MB
→ ботер отбраковывал meta → exit 0x91"；Turing 的输入载体 mailbox 让我们
连 0x91 都看不见）。三处偏差：
1. **gspFwHeapSize**：我们的 layout 推导值（88MB 档）vs 驱动精确常量
   （8GB→0x6900000，10GB→0x7F00000，20GB→0xFE00000）；
2. **flags(241)**：0 vs 0x1（CLOCK_BOOST，活体 dump）；
3. heapOffset 必须 = (gspFwOffset - heapSize) 对齐，**不得**从对齐后
   offset 反推 size（v2.73：驱动在 heap-end 与 gspFwOffset 之间留
   0xA0000 间隙）。

**决定性交叉验证**：用我们的 gspFwOffset + 驱动公式 → heapOffset =
**0x1f7900000**，与 40HX 8GB 真机 dmesg 的 gspFwHeap=0x1f7900000+0x6900000
**逐位相同**。

**修复（已入库）**：wpr_meta_pack 增 family 参数——tu10x 用驱动精确
heap 三元组 + flags=0x1；Ada/GA10x 路径逐字未动。

**附注**：此前 replay 的"verdict ok"（EXCI=0+WPR2+PLM）无法区分 meta
被收下还是被拒——BOOT_DIRECT 无状态口。本修复后 payload 起爆与否
（PLM 读回）才是 meta 是否被接受的唯一判据。

---

## attempt 9（2026-10-27，log: attempt9-payload-same-signature.log）

heap 修复后 payload 结果与 attempt 8 **完全同签名**（HALT/EXCI=0/PLM 锁定）。
两可能：修复未上车，或还有其他 meta 字段被拒——上一轮的观测量无法区分。

**新增两件观测仪器（已入库）**：
1. `WprMeta tu10x driver-exact heap: ...` 部署标记行——下一火 log 必须出现
   此行才能证明新 pack 在跑（banner 时间戳已证明不是可靠的 freshness 指标）；
2. **DMEM 取证读回**：HALT 后经 DMEM 端口读 0x600/0x6140/0xFF38/0xFF4C/
   0xFF50/0x200 六个字。判读表：
   - `DMEM[0x600]=0x000004a7`（载荷填充字）且 `0xFF38=0x00006140`（帧 guard）
     → **签名 DMA 发生了**（meta 被收下）→ 若 PLM 仍未开 = canary/gadget 层
     问题（同镜像+钉版载荷在 40HX/50HX 上成立，此分支概率低）；
   - 六个字都是 booter 自身初始化残留 → **meta 仍被拒** → 下一个嫌疑字段：
     bootloaderCodeOffset/DataOffset/Manifest（我们 desc 派生 vs 50HX 活体
     dump 的 0x1800/0x800/0x0）、sizeOfBootloader（我们 4KB v4 图 vs 他们
     128KB debug blob）、sizeOfRadix3Elf（我们 29MB 真图 vs 他们 dummy）。

---

## attempt 10（2026-10-27，log: attempt10-dmem-forensics-noexec.log）——取证仪器立功，M3"闭环"撤回

部署标记在场（heap=0x1f7900000/0x6900000 ✓ 新 pack 在跑）。DMEM 取证
六字全零（0x600/0x6140/0xFF38/0xFF4C/0xFF50）——签名 DMA 没发生；栈区
零残留=**booter 从未执行**。DMEM[0x200]=0x67436032 是我们 portload +
SIG_PROD 补丁的回显，非运行痕迹。

**根因（自摆乌龙，仪器抓住）**：`booter_fire_direct` 的 START 写的是
**bit0（0x1）**——早期 stub 遗留值；真 START=**bit1（0x2）**
（`NV_PFALCON_FALCON_CPUCTL_STARTCPU_TRUE`，cmp50hx:353 一手；我们的
fwsec_ignite/booter_fire 也都是 1<<1——GSP 因此 3ms 点燃，SEC2 因此三次
"HALT"全是 reset 后本来就 halt 的平凡满足）。**attempt 6/7 的"M3 闭环"
是假阳性**：EXCI=0/WPR2/PLM 三判据被"从未启动"平凡满足——已撤回；
AD107M 轨上该路径用的是 booter_fire（bit1）故 M4 真实闭环，未受影响。

**修复（已入库）**：START=0x2 + alias 探针 + START-drop 探针（仿
booter_fire），booter_fire_direct 与 booter_fire 尾部现逐段等价；sim
行为模型同步 bit1（47 场景全绿）。

**attempt 6/7 的其它观察仍然有效**：mailbox=输入载体语义（DMEM 取证与
之自洽）、FWSEC 路径三次复现、PLM 锁定态观察、heap 修复**未接受真机
检验**（booter 没跑过，meta 收没收下仍是未知数——下火 DMEM 取证才首次
真正检验它）。

**下一火预期**（首次真实的 booter 执行）：
- start-probe 行应显示 imm≠pre（bit1 锁存）；
- 若 meta 收下：DMEM[0x600]=0x4a7 + PLM 开 + payload 判据 → selector
  写入 → SELECTOR RESULT → 完整解锁链；
- 若 meta 仍拒：DMEM[0x600]=0x4a7 但 PLM 锁 → canary/gadget 层（同镜像
  钉版载荷在 40HX/50HX 成立，低概率）→ 对照 desc 差分；
- 若 DMEM 全零依旧：START 仍被吞 → alias retry 行给出判据。

---

## attempt 11（2026-10-27，log: attempt11-booter-executed-sanitize.log）——**booter 首次真实执行**

START=bit1 修复生效：`start-probe: pre=0x10 imm=0x0`（核心离开 halt 态）→
执行 → 停机。mailbox0/1 被清零（输入载体被 finalize 擦除）——**这是与
"从未启动"（mailbox 保留输入值）的判别性差异**。

**sanitize 现场发现**：DMEM 六字全部 = **0xdead5ec2**（"dead sec"）——
booter 收尾的 sanitize 步把全部取证痕迹擦掉了。attempt 10 的"全零"与本次
的"全 dead5ec2"都是 booter 未到/已擦的状态，**post-halt 读回永远抓不到
DMA 瞬间**。EXCI=0x00900100（对照 GSP 常态 0x00900300，语义未定，留档）。
WPR2_HI=0x0：payload 第二写或 booter 自身收尾，不可分辩。

**下一轮改造（已入库）**：
1. **实证 BL 输入集**：gsp_boot_tu10x.bin ← GspRmBoot 镜像前 0x6000 字节
   （GSP_RM_BOOT_SIZE=0x6000，50HX 源码 476 行）+ 拓扑钉版
   code@0x1800/data@0x800/manifest@0x0（活体 dump）——我们的 4KB v4 图
   从未被任何 booter 消费过，全面换用 50HX 实证输入；
2. **运行中 DMEM 采样器**：START 后每 500μs 采 DMEM[0x600]——0x4a7 出现
   = 载荷落地（meta 被收下）实锤；0xdead5ec2 出现 = sanitize 先到
   （meta 被拒）实锤。不再让 sanitize 毁灭证据。

## 状态重申（诚实账本）

- 真实可信：FWSEC 路径（三次复现）、booter 执行机制（bit1 修复后首证）、
  layout/heap 公式、mailbox 语义、sanitize 行为。
- 待检验：meta 是否被收下（heap 修复 + BL 输入集的有效性）、ROP 是否
  起爆（gadget/canary 层从未被真实到达过）。
- 已撤回：attempt 6/7 的"M3 闭环"。

---

## attempt 12（2026-10-27）+ **Turing FEAT 地址全案告破**

`sanitize reached DMEM[0x600] at sample 0`——采样器 5ms stall 后才起跑，
booter 全生命周期 <5ms 就完成了 sanitize。**但三件观测拼出了真答案**：
①WPR2_HI=0x0（= payload 第二写 WPR2_HI=0 的目标态！）②PLM"未开"（读的
0x823804）③booter 报成功（mailbox0=0）。

**根因：Turing 的 FEAT 块根本不在 0x823xxx。** 50HX 源码 294-296 行铁证：
`FEAT_PLM=0x00409650 / SS0=0x00409664 / SS1=0x0040966C`（payload 第一写
目标 0x409650 与之自洽）。我们的 M1 "FEAT 簇 13/13 badf5040" 测的是
0x823xxx 的**另一个锁定块**；verify/handoff 读的也是错块。

**推论：attempt 11/12 的 payload 很可能已经完整起爆**——PLM 在 0x409650
被打开（AON，FLR 后应存活）、WPR2_HI=0 落地，而我们读错寄存器判失败、
FLR 后把活解锁扔了两次。验证途径：①Windows 若未断电——FP16/FP32
benchmark 对 stock（AON 存活则当前会话就是解锁态）；②下火直接重跑。

**地址修正（已入库，全部路径）**：verify_execution_tu10x → 0x409650/64/6c；
selector 写（含 force-continue 探针）→ 族感知；flr_phase 增 family 参数
（pre/post-FLR selector 读用 Turing 地址）；handoff_out 双块并读；M1 采样
表增 TU10x FEAT_PLM/SS0 两行（冷态应 badf5040=确认块存在且锁）。开放问题
#1 的"答案"需改写（0x823xxx 是别的块）；#10 关闭路径更新（PLM 开后扫
0x409600-0x409700 找出厂钳制字段——代码已加）。

---

## attempt 13（2026-10-27，log: attempt13-payload-fired-selectors-stuck-flr-relock.log）——**payload 完整起爆，差最后一步**

全链真实执行到位：
- M1 新行：`TU10x FEAT_PLM 0x409650 = 0xffffff8f [ok]`——**真 FEAT 块 PL0
  可读**（"Turing PL0 全锁"是测错块的伪象，开放 #1 改写）；冷值 SS0=0x15000
  /SS1=0x1（出厂态）。
- payload 起爆：`POST-BOOT: FEAT_PLM=0xffffffff`（0x409650 真地址）→
  **SELECTOR RESULT: SS0=0x88888888 SS1=0x8**（写入+读回通过）→ FEAT 块
  扫描全簇落盘（0x409650=0xffffffff / 0x409664=0x88888888 / 0x40966c=0x8
  逐位确认）→ 原厂签名恢复 → pre-FLR selectors 保持解锁值。
- **最后一步失败：FLR 自己重锁了 PLM**。post-FLR `plm=0xffffff8f`、SS 读
  不可达（badf→0）。**Turing FEAT PLM 非 AON**——与 Ada（AON 存活、M4 靠
  此闭环）为第三个代际差异。而 cmp50hx 链**本无 FLR 步**，其解锁活着进
  OS（zero Xid）正因为没有这一步。

**修复（已入库）**：①tu10x 跳过 FLR（booter 自身 sanitize=清理，cmp50hx
形状）；②claim EALREADY 换族感知地址（0x409650）——已解锁的卡必须
skip，绝不覆盖重放（da96e3f 语义）；③上轮回复中"attempt 11/12 可能仍
解锁"的说法被本火证伪（PLM 被 FLR 重锁），以此为准。

**出厂钳制第一手数据（冷值）**：SS0=0x15000（4 位字段 f3=5,f4=1）/SS1=0x1，
对比解锁值 0x88888888/0x8——TU106 出厂 SS 非默认值，**钳制谱解码与收益
测定等 Windows benchmark**（本火 selector 已写成功但被 FLR 抹掉）。

**下一火（FLR 跳过版）预期**：全链到 SELECTOR RESULT 后**直接 HANDOFF-OUT
（tu10x: plm=0xffffffff ss0_unlock=true）**→ 链回 Windows 带着活解锁 →
benchmark 定收益。

---

## attempt 14（2026-10-27）——**解锁本体闭环，Windows 交接暴露最后缺口**

payload 完整起爆 + **FLR 跳过生效**：HANDOFF-OUT
`tu10x: plm=0xffffffff ss0=0x88888888 ss1=0x00000008`——解锁活着交棒。
FEAT 块扫描再次全簇确认。M1 冷值本轮 SS0=0x0（上一火 FLR 把 SS 归零而非
恢复出厂 0x15000——FLR 后 FEAT 覆盖寄存器归零的又一数据点）。

**新问题：Windows 显示驱动初始化失败（Code 43），内核级卡死 1 分钟
（黑屏/键盘灯灭/硬盘灯活跃），休眠与冷重启复现，设备管理器禁用/启用
同样暴死。** 详见 reply 差距分析。

---

## Run B/C 矩阵结果（2026-10-27）+ attempt 16（PLM re-lock）

- **Run C（replay，无 PLM/SS 写，无 FLR）：Windows 稳** → 无 FLR 交接残留态
  （SEC2 sanitize/GSP 半态/WPR2 半拆/GFW 未完成）全部可存活——排除。
- **Run B v1 作废**：`\tu10x-no-ss.txt` 的 `\t` 被 Rust 转义成 TAB（路径
  转义地雷第三次，教训再确认：此类代码只用 Edit 工具+二进制探针），SS 写
  照常执行=跑了又一个 Run A。
- **Run B v2（marker 生效，PLM open、SS 保持出厂 0x11000/0x1）：仍死卡**
  → **SS=0x88888888 无罪。凶手=PLM open 本身**（0x409650=0xffffffff 在
  驱动初始化时在场，A/B 死；C 不在场，活）。
- 新线索：SS0 冷值三连漂（0x15000→0x0→0x11000）——SS 寄存器本为动态
  （AD107M 同款引导间漂移），写死值的语义需隔离验证——已由本矩阵完成。

## attempt 16：PLM re-lock 版（"开门-写字-关门"）

PLM 只是写权限门，覆盖值才是载荷。selector 写完后把 0x409650 写回
0xffffff8f（锁定基线）并读回三件套：plm 是否回锁 + SS0/SS1 解锁值是否
在关门后存活（unlock_persist 布尔落日志）。

**判读**：persist=true → 驱动见到"重锁但被改值的卡"（cmp170hx 原厂重锁
值同款）→ benchmark 定收益；persist=false → 覆盖与门绑定，解锁必须门开
→ 而 open 态驱动拒绝=CMP 参考全靠无头卡（无显示驱动重验证此块）的空档，
消费显示卡此路不通，TU106 轨以"机制全通、消费显示卡无可交付解锁"考据
收官（机制本身仍是 M3/M4 全链首创成果）。

---

## 六仓参考增援（2026-10-27，全部 clone 入 reference/）——**Windows 显示卡实证 + Code 43 终解**

| 仓库 | 芯片/形态 | 关键贡献 |
|---|---|---|
| **chenxluo/tu102-22g-uefi-unlock** | **TU102 2080Ti 22G 改装卡（10de:1e04，Windows 显示卡）** | **+51~75% FP16/FP32 累加实测**；README 明示：无 POST 直通 Windows，**由 `EnableGpuFirmware=1` 驱动模式接受 pre-OS 状态**；注册表路径 `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\<卡>` 下 `EnableGpuFirmware=1`(DWORD)；同款 cmp50hx 基座 |
| mirabarukaso/nv_unlock_win | Turing/Ampere/Ada 三合一 EFI 载具（EDK2） | Titan RTX（满速卡）Windows 实证：注入成功但无收益（出厂即满速）——与"SS 值无害"观察一致 |
| miounet11/gpuunlock | Ada 参考 + register report | 与 gpu-unlock-ada 同源 |
| rough2515-hub/rtx3080-unlock-public、craftingmod/rtx30-unlock | GA10x | Ampere 谱系补充 |
| Siselea/rtx30-unlock-windows-toolkit | **单卡单显 Windows 11 工具包** | v20 GOP "quiet profile" 单显示器补丁——与我们机型同类的周边工艺 |

**用户关键提醒（一手事实）**：40 系（Ada）Windows **不要求** GSP 模式——我们 M4
（28e0 无注册表设置）就是证明。EnableGpuFirmware 是 **Turing Windows 驱动
路径的代际特性**：Turing nvlddmkm 的 legacy 初始化路径不接受/不接受住
pre-OS FEAT 态（撞开放 PLM 即 TDR），GSP-RM 路径才接受。

**对齐我们全部观察**：A/B 死（PLM open 在场+legacy 驱动）⇔ C 活（PLM 锁）
⇔ chenxluo 卡活（PLM open 在场+GSP 模式驱动）⇔ M4 活（Ada 无此要求）。

**attempt 16 的 re-lock 版仍值得跑**（读回 unlock_persist 判 SS 覆盖是否
门绑定），但主路径已明确：**注册表 EnableGpuFirmware=1 + 现有解锁 EFI**。

---

## attempt 16 + GSP 模式终局（2026-10-27）——**TU106 轨完整闭环**

- **attempt 16（PLM re-lock）失败**：关门后 SS 覆盖不存活 → **SS 覆盖与
  PLM 门绑定**——解锁必须门开交付，没有"重锁态交付"这条捷径。
- **Windows GSP 模式（EnableGpuFirmware=1）+ 现有解锁 EFI + 重启冷启 =
  完整成功**：36 → **60~63 TFLOPS（+67~75%）**，落在 chenxluo 2080Ti 参照
  带（+51~75%）内。TU106 轨从 M0 到收益全程闭环。
- **机制总图**：Turing Windows 驱动的 legacy 路径不接受 pre-OS FEAT 态
  （撞 open PLM 即 TDR）；GSP-RM 路径接受。Ada 无此要求（M4 无注册表
  闭环）。SS=0x88888888 本身无罪（Run B v2 已隔离）。
- **收尾改动**：GOP 重连恢复为默认开（TDR 真凶是驱动路径非 GOP devinit；
  Run A 的形状变化实为巧合），`\tu10x-no-reconnect.txt` 可再关；新增
  `tools/windows/enable-turing-unlock.ps1`（枚举 {4d36e968} 类下
  MatchingDeviceId 的 Turing 设备 ID 段 0x1E02-0x1FFF/0x2182-0x2192，
  写/删 EnableGpuFirmware，含 admin 检查与"必须 Restart 而非休眠恢复"
  提示——hibernate 恢复不重初始化驱动，GSP 切换不生效且旧会话会 TDR）。
