# 开放问题清单（按关闭成本排序，cmp170hx 惯例）

| # | 问题 | 状态 | 关闭路径 |
|---|---|---|---|
| 1 | ~~TU106 FEAT 簇是否存在/PL0 可读~~ | **已回答（2026-10-26）：存在，但 Turing 在 PL0 全锁定**（13/13 PRIV_VIOLATION，非 UNDECODED/FLOORSWEPT）——与 Ada 全可读形成代际差异；Turing 解锁需先解 PLM | 已关闭 |
| 2 | ~~AD107M 是否存在算力钳制~~ | **已回答（2026-10-25）：存在。** FEAT_READOUT_1=0x00410080（字段 2/5/7=2）；FEAT_OVR_PLM=0xffffff8f 锁定基线；熔丝总开关 0x0 未烧 | M4/M5 立项时的输入 |
| 3 | 0xbadf5620（AD107 SEC2_CPUCTL）的语义 | **部分收敛（2026-10-25 考据）**：badf 毒值结构 = `0xBADF<错误码8位><附加信息8位>`（一手证据：kernel_sec2_gb10b.c 命名 0xBADF4100=privErrTargetLocked 且注释"LSB 携带额外错误信息"；kernel_falcon_ga102.c 以 0xbadf0000 为泛化基值）。已命名/社区已证码：0x41xx=目标锁定、0x5040=特权违规、0x5108=安全 PLM 主机读、0x20xx=floorswept（物理不存在）、0x12xx=PRI 复位中、0x1100=PRI 毒值。**0x56 码无任何公开定义**——落在 0x5x 访问限制族（推断：冷态 SEC2 域受门控/访问限制，非"不存在"——若 floorswept 应为 0x20xx）。**0x13xx（2026-10-26 新测：28a1 的 VGA_WORKSPACE，dGPU 非主显）**同样无公开定义，低位 0x01 提示"未配置"而非受限 | 收敛路径：AD102 debug booter 反汇编查 0x56/0x13 处理，或 Level 2 实验观测（引擎复位序列，若仅是门控态则复位后应可访问）；0x13xx 可用"MUX 切 dGPU 主显后重跑"做对照 |
| 4 | TU106 GSP（RISC-V）的 Falcon CPUCTL 界面是否同构（AD107 侧：CPUCTL=0x10 HALTED 可读） | **部分关闭（2026-10-26 TU106 采样）**：GSP_CPUCTL 0x110100=0x10 PL0 可读、HALT 位同义；SEC2_CPUCTL 0x840100=0x10 亦可读（无 AD107 的 badf5620 门控）。剩余：BCR 0x668 RISC-V/Falcon 切换语义（内核 `kflcnResetIntoRiscv_HAL`） | falcon_reset 首跑观察 BCR 前后值 |
| 5 | uefi-rs 对 GOP 断连（DisconnectController）错误码保真度 | 开放 | M3 前 QEMU+实机验证；已知 GOP 句柄≠GPU PciIo 句柄（AD107 实测） |
| 6 | TU106 台式机 Secure Boot 现状 | 开放 | TU106 实机前现场查 BIOS |
| 7 | ~~WPR2 冷态判据措辞修正~~ | **已回答（2026-10-26 双机交叉）**：WPR2_LO=0x1ffffe00/HI=0 在 AD107M 与 TU106 冷态**完全一致**——是 GFW 后空窗口默认态，非"本周期已初始化"；任务书 §3.2"恒 0"应读作"**hi 恒 0**"。守卫已按 hi≠0 实现并双机验证放行 | 已关闭 |
| 8 | ~~Ada 四写链寄存器在 AD107M 的存在性与冷值~~ | **已回答（2026-10-25 Level 1 采样）**：三寄存器全在位；WPR/WPRCFG=0x0004cb8f（=cmp170hx 原厂重锁值模式）、FBPA=0xffffff0f（=任务书 §7.4 社区观察值） | Level 2/3 输入 |
