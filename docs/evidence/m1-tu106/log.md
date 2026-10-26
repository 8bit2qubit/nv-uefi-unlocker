# TU106 (RTX 2070, 10de:1f07) M1 cold sample — 2026-10-26

PMC_BOOT_0 = 0x166000a1 (TU106 ✓)
FEAT cluster 0x823800-2f + 0x823b00: 0 ok / 13 badf5040 (PRIV_VIOLATION)
  -> OPEN QUESTION #1 ANSWERED: cluster EXISTS on Turing (priv violation,
     not undecoded/floorswept) but is PL0-LOCKED. Ada = fully readable.
FEAT_READOUT_1 = badf5040 (cannot determine throttle state at PL0)
FB_SIZE_MIB = badf5040 (locked; Ada readable)
MMU_PLM = badf5040 (locked)
SEC2_CPUCTL = 0x00000010 READABLE/HALTED  (AD107: badf5620 gated!)
GSP_CPUCTL = 0x10 halted
PGC6_BSI_SCRATCH15 = 0x202001bf -> handoff = 1 (same as AD107)
FUSE cluster (0x824140/1c0/0x8203f0) = all badf5040 (locked; Ada readable)
FBPA_PLM 0x9a0148 = 0xffffffcf (readable, value differs from AD107 0xffffff0f)
ADA WPR/WPRCFG PLMs = badf5040
MMU_LOCK_LO/HI = 0x00100000/0x01effff0 (same shape as AD107)
WPR2 = 0x1ffffe00 / 0
NV_PROM first 64K crc32 = 0x552e185c, magic 0xeb75aa55 (card identity)
VGA_WORKSPACE = 0x1

PCIe: FLR capable, LINKCAP 0x453d03 (Gen3 x8)
Post-FLR: gfw_boot = 0x101 STUCK (does not re-complete in 10s) —
  IDENTICAL to AD107. Cross-generational FLR behavior.
BSI handoff post-FLR: still 1.

Transaction: stale EFI (pre-multi-family) attempted AD102 assets on
TU106 — claim guard EIO (FEAT_PLM unreadable) STOPPED the cross-family
accident. Constitution guard design validated.

## Run 2 (2026-10-26, EFI sha256 6b6d3095..., multi-family mapping fix)

Device 10de:1f07 -> asset family 'tu10x' (mapping fix CONFIRMED on HW).
\booter_load_tu10x.bin loaded 438321/438321 (that file was the raw
generated .c source, not yet a bundle — superseded by extract-tu10x v2
which now emits the parsed 59224B BTL1 bundle, pins frozen).
gsp_boot_desc/gsp_boot/fwimage/fwsignature_tu10x NOT_FOUND -> graceful
"assets missing - skipping" -> CHAIN-GATE -> bootmgfw chainload clean.
No writes reached the GPU. M1 SAMPLE section identical to run 1.
