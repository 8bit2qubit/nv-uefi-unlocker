
## Run: TX replay round 2026-10-26 (EFI ea2098c2) — scrubber forensics + PORTAL BUG FOUND

DETACH+claim ok (GOP 7->4 protocols, PCI 5->4, EXCLUSIVE, cmd=0x0007).
PRE-FLR ran (flag didn't exist yet; opt-in only from next build).
Scrubber attempt: app 0x49 (ucode_id 10, imem 0x5800 dmem 0xc28 pkc 0xa10,
sig_versions 3, nsig 2). SEC2 fired -> HALT. Forensics:
  EXCI=0 TRACEPC=0x633aa IMEMC0=0x510015c0 DMACTL=0x1
  DMEM@0x10: 002009ac 28e00003 00000000 02080401 ...

CORRECTION (supersedes the "device-ID echo proves execution" reading):
0x28e00003 is STATIC image data. Static-vs-runtime byte comparison shows
the dump was SHIFTED 12 bytes: DATA_INDEX was written addr>>2 while the
v4 encoding is the raw byte address (dev_falcon_v4.h: DMEMC OFFS[7:2]
BLK[23:8]; kernel kflcnMaskDmemAddr = addr & (OFFS|BLK)).
Runtime DMEM == static image bytes => NO evidence the ucode executed.

ROOT CAUSE (same 4x scale bug, IMEM side): portal_load_imem wrote
(addr>>2) into IMEMC - IMEM block k landed at byte 64k instead of 256k,
four blocks overlapped per 256B span; IMEMT tag was also <<8-shifted.
Every portal image load beyond block 0 was corrupt in ALL prior rounds.
Zero-address DMEM ops masked it (loads at DMEM 0 verified "ok").

FIXES (build a7cd7e33...):
- tx_portal.rs: authoritative v4 encodings (addr & 0x00ff_fffc; TAG[15:0]
  raw; AINCW bit24; SECURE bit28); portal_read_dmem helper added.
- scrubber forensics: CPUCTL post-halt, DMEM top window (stack area),
  fuse_raw/version/sig_slot logging.
- PRE-FLR now opt-in via \pre-flr-on.txt (default OFF = kernel-matching
  flow; post-FLR GFW-stuck environment never occurs in the kernel path).

## Run: fixed-portal round 2026-10-26 (EFI a7cd7e33) — CPUCTL=0x12 STARTED, FBIF gap found

Portal encoding fix VERIFIED live: IMEMC0=0x11005800 decodes as
SECURE|AINCW|BLK=88 (all 88 blocks placed, image intact for the first
time). CPUCTL post-halt = 0x12 = STARTED|HALTED: the core genuinely
started and halted. EXCI=0. Handoff still 1.

New finding A - DMEM read path: the 64-word dump returned the SAME word
repeated (0x02080401 = static image dword at 0x10): AINCW auto-increments
writes only. Reads now seek per word (portal_read_dmem).

New finding B - root cause of the halt-without-progress: DMACTL read back
0x00000001 = REQUIRE_CTX set. Our scrubber path (portal rewrite) dropped
the unlock_610.c falcon_dma_load FBIF trio (FBIF_CTL|1<<7 ALLOW_PHYS_NO_CTX,
DMACTL=0, TRANSCFG(0)=(x&7)|5). The scrubber's own runtime FB-DMA translates
through that FBIF - with ctx required every fetch dies. Likely also the
original "2x32-slot queue accepted, never consumed" stall cause (now also
present in falcon_hs all along? no - falcon_hs HAD the trio; the GSP-side
stall had other portals unconfigured - to re-examine).

New finding C - WRONG UCODE: the kernel's scrubber is the driver BINDATA
g_bindata_ksec2GetBinArchiveSecurescrubUcode_AD10X (image 7424B, ucode 0xd,
2xRSA-384), NOT the VBIOS PROM app 0x49 (22528B) we were firing. Extracted
via build_bundle -> scrubber_ad10x.bin (8264B BTL1, pins frozen:
src 94dbc822..., derived 1cfc7813...). Also: our scrubber fuse read used
register 0x8241c0 (wrong); kernel ksec2ReadUcodeFuseVersion =
FUSE_OPT_FPF_SEC2_UCODE1_VERSION 0x00824140 + 4*(id-1), version=highestbit+1
(booter path already correct at 0x4140).

FIXES (build ce59723b...): falcon_dma_unblock() (trio + read-back gate)
called in the scrubber path; scrubber switched to kernel-exact BINDATA via
load_booter("scrub") + booter_patch_stock + BROM arming + mailbox0
read-back; per-word DMEM reads.

## TOTORONG cross-check (2026-10-26, reference/AD103_win_unlocker)

Transaction body = community unlock_610.c compiled into EFI via compat
shims - chain shape matches ours by construction. Four carrier-craft gaps
adopted (FLR devctl preservation, Transactions-Pending wait, 100ms FLR
silence, restore ordering without BME); FBPA nonfatal verdict + badf5620
poison check independently confirmed in their VerifySelectors (their code
guards ss0/ss1 against 0xbadf5620 on AD103 too - open question #3
converges further: cross-family gated-domain poison).

## Run: kernel-scrubber + FBIF-unblock round (EFI d56f78b3) — verdicts

FBIF domain NOT gated: falcon_dma_unblock read-back passed on BOTH engines
(GSP: CTL=0x190 TRANSCFG0=0x115 DMACTL=0; SEC2 same in scrubber path).
The gated-FBIF hypothesis is dead.

Scrubber (kernel-exact BINDATA, ucode 0xd): SEC2 CPUCTL post-halt = 0x10
(no STARTED bit), MAILBOX0=0, EXCI=0x01000100, and the DMEM@0 dump is
BYTE-IDENTICAL to static signature slot 0 (verified host-side against
scrubber_ad10x.bin) -> the ucode NEVER EXECUTED. The BROM path held the
core (last round's raw start of corrupt IMEM showed 0x12 started — the
0x12 came from OUR start write executing garbage, not from BROM).

GSP falcon DMA (community falcon_hs sequence): accepts exactly 64
commands (16 KiB = 2x32 slots, matches CAPS fifo) then stalls; DMACTL
self-sets undefined bits 0xd0000. Not ctx (DMACTL=0), not FBIF (verified),
not VT-d. Engine-private state; bypassed rather than reversed.

CHAIN PROGRESS: layout computed from registers (fb=0x1ffc00000, frts=
0x1ffa00000) and FWSEC reached for the first time in the no-pre-FLR flow.
Post-transaction FLR line: gfw_boot=0x1 (was 0x101 in pre-FLR rounds).

NEXT: fwsec_ignite switched to portal placement (build 47f49aaf...) —
same kernel HS tail (PKC params, BOOTVEC, start); FWSEC on the GSP BROM
is the second independent BROM-validation observation point.

## Run: portal-FWSEC round (EFI 47f49aaf) — MILESTONE: FWSEC fired, WPR2 verified, booter stage reached

FWSEC via portal on the GSP BROM: PASSED validation and ran —
  FWSEC portal forensic: CPUCTL=0x10 MAILBOX0=0 EXCI=0x00900000
  FWSEC WPR2: low=0x1ffa000 high=0x1ffae00 (== computed frts page)
  FWSEC: WPR2 verified
BROM HS validation WORKS in our window on the GSP. The scrubber's
SEC2 BROM hold is therefore SEC2-scrubber-specific, not a window-wide
condition. FWSEC also fired with handoff=1 — community chain shape
confirmed on silicon: no scrubber needed.

Booter stage reached for the first time: bundle parse ok (56832/768),
stock sig patched (fuse_ver=1), 84 MiB radix built (20606 pages),
five DMA blocks allocated. Failed EInval at wpr_meta_pack — ROOT CAUSE
host-side: layout_from_registers was fed image_size = gsp_boot len
(36864) instead of the GSP ELF payload (84226048); the gsp slot was
sized 5.8 MiB and the pack check refused. Fixed: layout now receives
assets.gsp_image.len(); slot recheck = image + 8 KiB slack (passes).

PREEMPTED next wall: booter_fire switched from falcon_hs (DMA) to
portal placement — the SEC2 DMA engine would hit the same 64-command
wall. Kernel BootFromHs tail kept verbatim (IMEM@phys0 va=app_code_off
secure, DMEM@0 with in-place signature, PKC params, BOOTVEC, mailboxes=
WprMeta, start, HALT + CPUCTL/MAILBOX0/EXCI forensics).

## Run: portal-booter round (EFI b4fa8547) — SEC2 BROM holds the booter too; payload question deferred

Full publication path GREEN for the first time: layout 187 MB WPR
(0x1f5000000-0x1ffb00000) with the 84 MB image slot, WprMeta published,
booter portal-placed (125+1 IMEM blocks, 24 KiB DMEM), signature slot 0
(fuse_ver 1). Then: BOOTER forensic CPUCTL=0x10 (no STARTED)
MAILBOX0=0x38300000 (= OUR meta address, untouched) EXCI=0x00900100.
SEC2 BROM held the booter exactly like the scrubber.

Split confirmed across two different SEC2 ucodes:
  GSP BROM: PASSES HS validation (FWSEC ran twice, repeatable)
  SEC2 BROM: HOLDS (scrubber 0xd AND booter 3)

PAYLOAD DECISION: firing V67 now would be technically void - the payload
differs from stock only in signature-slot bytes and rides the identical
SEC2 launch path that is currently blocked; M4 gating (AGENTS 1.7)
applies on top. Order of operations: unblock SEC2 BROM -> complete M3
replay -> then payload as the gated separate step.

Next-round probe added (EFI c0fce9ac): immediate CPUCTL read-back after
the START write + alias-path retry. If the write is dropped (pre==imm),
a PLM-class gate on SEC2 CPUCTL writes is proven and redirects the
investigation; if it takes and the core still holds, the BROM rejection
is real and signature/fuse-domain side needs the microscope.

## Run: start-probe round (EFI c0fce9ac) — START write DROPPED on SEC2

BOOTER start-probe: pre=0x10 imm=0x10 +5ms=0x10, alias(+0x130) retry -> 0x10.
The SEC2 CPUCTL START write is silently dropped on both paths, while the
identical write starts the GSP core (FWSEC, twice). SEC2's other registers
accept our writes (MAILBOX0 readback = our value; FBIF/DMACTL/portal
verified). SEC2 is not block-gated - the gate is precisely on CPUCTL,
the engine-start authority (read: badf5620; write: dropped). BSI handoff
still 1. Full walkthrough matrix committed as docs/chain-walkthrough.md.

## Run: AUTH-SCAN round (EFI 62a37a05) — the pre-reset gate map is SYMMETRIC

AUTH-SCAN (as-booted, both engines): BOOTVEC/DMACTL/DMATRF*/CPUCTL_ALIAS/
TRACEPC/IMEMC/DMEMC/falcon2-BROM regs are badf5620 on BOTH GSP and SEC2 -
the pre-reset register gate is the normal domain lock, not SEC2-specific.
FWSEC booted from exactly this state, so all decisive operations happen
POST falcon_reset (which demonstrably releases the gates: IMEMC readable
after reset).

Real asymmetries found:
- HWCFG +0xf4: GSP=0x47b7 vs SEC2=0x67b7 - bit12 (scrub-wait, AGENTS 7.2)
  set ONLY on SEC2 in the as-booted state.
- +0x3c4 (TOTORONG reset-PLM candidate): GSP=0xfffffbff (bit10 clear) vs
  SEC2=0xffffffff.
- falcon2 window inverted: SEC2 BCR/RISCV388 readable pre-reset, GSP's
  gated (GSP's BCR only became readable post-reset inside falcon_reset).
- +0x300: badf5108 on BOTH (secure-PLM poison class - register exists,
  PLM-protected).

Conclusion: the SEC2 START-drop is a POST-RESET condition. Next probe
(build 7f79dc9a): post-reset mini-scan (CPUCTL/ALIAS/DMACTL/BOOTVEC/EXCI/
HWCFG/ENG_RESET/0x3c4) on BOTH engines + a SAFE CPUCTL write-stickiness
test (ALIAS_EN bit6 without START). If bit6 sticks on SEC2, writes work
and the START bit is specifically gated (-> alias start, auto-attempted);
if not, the register is fully write-protected post-reset on SEC2 while
writable on GSP.

## Run: post-reset probe round (EFI 7f79dc9a) — SEC2 CPUCTL WRITES WORK; START bit specifically dead

AUTH-POST GSP: write-probe 0x10 -> 0x50 bit6 STUCK; FWSEC then started
via the ALIAS path (probe left bit6 set) and WPR2 verified - alias-start
validated on GSP as well.
AUTH-POST SEC2: write-probe 0x10 -> 0x50 bit6 STUCK - SEC2 CPUCTL writes
WORK. But START is dropped on BOTH paths: direct (prev round, bit6=0)
and alias (this round, bit6=1): pre=0x50 imm=0x50 +5ms=0x50, core never
runs (MAILBOX0 = our value untouched).
Also: SEC2 BOOTVEC (0x100) and EXCI (0x01000100) SURVIVED the booter's
engine reset - the reset does not clear these registers.

Conclusion: not write protection - the START bit specifically never
takes on SEC2. New prime suspect: our falcon_reset order resets the
engine FIRST and switches BCR (0x110 -> 0x01, RISC-V-selected ->
falcon) AFTER, with no reset cycle to latch the switch - if the start
authority is still wired to the RISC-V selection, START drops while
normal writes work. Fix (build 08f23813): reset-into-falcon - one more
engine reset cycle + scrub wait immediately after the BCR switch.

## Run: reset-into-falcon round (EFI 08f23813) — experiment FAILED, reverted

Post-switch reset cycle on BOTH engines: BCR REVERTED to 0x110 (the
reset wipes the switch) and the HWCFG bit12 scrub wait hung (>1s) ->
falcon_reset EIO -> scrubber AND fwsec never ran. Reverted to the
reference order (reset first, BCR switch after, no further reset) -
verified byte-identical to unlock_610.c falcon_reset (including poll
0x11 semantics vs our bit0-only check).

Source check: AD102 has no sec2 arch file - SEC2 inherits GA102's HAL
(bBootFromHs=TRUE, Falcon HS). Our approach matches the kernel.

Next-round probes (build 15534dba): BROM param read-back (PARAADDR/
ENGIDMASK/UCODE_ID/MOD_SEL after write - if the falcon2 window drops
these like START, the BROM boot fails cleanly), EXCI clear attempt
(SEC2 latches 0x01000100 across resets; GSP clears to 0), ENG register
post-reset read.

## Run: brom-readback round (EFI 15534dba) — ROOT CAUSE #2: WRONG IMEM PLACEMENT

Probe verdicts: BROM params latch perfectly (PARAADDR=0x10 ENGIDMASK=0x1
UCODE_ID=0x3 MOD_SEL=0x1 - exactly as written). EXCI write-0 does NOT
clear (0x01000100 sticky). BUT: after the START attempt EXCI CHANGED
0x01000100 -> 0x00900100 - the core DID start and instantly faulted
(matches the scrubber's TRACEPC stuck at 0x100 = the NS entry).

ROOT CAUSE: our BootFromHs IMEM placement was wrong. The kernel loads
ONLY the secure code segment and lands it at PHYSICAL 0 (dest=imemPa=0,
virtual tags from imemVa) - the NS/os-code part of the image is NOT
loaded at all. We loaded NS junk at physical 0 AND sec code at physical
0x100, and double-tagged va 0x100 (both blocks) - the core fetched the
NS junk at its boot vector and insta-faulted. GSP FWSEC never hit this
because the VBIOS image has no ns/sec split (we happened to load it
correctly). This also retro-explains the "0x12 started + garbage run"
round and every SEC2 halt since.

FIX (build 025a7dc5): booter_fire and the scrubber now load sec-only at
dest 0 per kernel BootFromHs. (booter_fire_direct keeps ns+sec - Turing
BOOT_DIRECT genuinely loads both, s_prepareHsFalconDirect.)

## Run: FROM_HS placement fix round (EFI 025a7dc5) — SCRUBBER RAN! handoff DONE!

MILESTONE: with sec-only-at-dest-0 placement, the kernel-exact scrubber
EXECUTED and completed: "SCRUB: handoff DONE - SEC2/GSP domains
un-gated" - BSI_SCRATCH15 advanced 1 -> 3 for the first time. The
IMEM-placement root cause is confirmed by the strongest possible
read-back: the privileged state machine itself responded.

FWSEC then ran (third WPR2 verification, identical frts page).

New failure, clean and narrow: after scrubber success SEC2's PRI window
re-locked (post-halt reads badf5620 - visible in the scrubber forensic:
TRACEPC/IMEMC/DMACTL/DMEM all badf5620) and booter_fire's
falcon_dma_unblock READ failed (EIO before its log line). Root: the
booter never performs its own engine reset - the kernel resets before
EVERY HS execute, and the reset is what re-opens the PRI window.

FIX (build cf8ec465): booter_fire now starts with falcon_reset(SEC2),
then dma_unblock, then portal placement, then fire - kernel order.

## Run: booter-reset round (EFI cf8ec465) — MILESTONE: BOOTER EXECUTED, M3 REPLAY VERDICT PASSED

- Scrubber: handoff DONE again (repeatable, second consecutive run).
- Booter: falcon_reset(SEC2) re-opened the PRI window post-scrubber
  (AUTH-POST SEC2 all readable), portal placement sec-only@dest0,
  BROM params latched, START via alias path TOOK: CPUCTL 0x50 -> 0x40
  -> 0x10 (started, halted), MAILBOX0 = 0x00000000 = stock booter
  SUCCESS (kernel semantics: nonzero = failed), WPR2 still up.
- "TX: execution verified" - the chain-replay verdict PASSED:
  the stock AD102 booter booted GSP-RM from our WprMeta/radix/sysmem
  publication, in the UEFI pre-driver window, on AD107M.
- FEAT/SS remained stock (0xffffff8f / drift values) - correct for
  replay mode (no unlock writes).

Remaining: flr_phase failed at its first-ever real invocation - the FLR
clears the BARs and the phase polled GFW_BOOT via BAR0 without
restoring them (poll inaccessible 0xffffffff). FIX (build deeb3bca):
pci_save at phase start + restore (decode off, high BARs first, command
&~BME last) after the FLR identity wait - unlock_610.c pci_save_state/
restore_state semantics.

## Run: flr-restore round (EFI deeb3bca) — execution verified AGAIN (repeatable); last wall = post-FLR HALT timing

- Scrubber handoff DONE (3rd consecutive), booter fired (2nd
  consecutive), MAILBOX0=0, WPR2 up, "TX: execution verified" - the
  replay verdict is now repeatable.
- flr_phase BAR restore WORKED (cmd=0x0003 restored, BAR polls live).
- Final miss: post-FLR "GSP not halted (0x0)" - the chip was mid-reboot
  (GFW restarts, WPR2 was up so the reboot path differs from the old
  stuck-0x101 experiments); the immediate HALT assertion fired before
  the cores settled.
- FIX (build 926d5568): post-FLR GFW_BOOT poll tolerant + HALT poll with
  a 10s budget, timeout logged but not fatal (the FLR itself is the
  hardware state guarantee). WPR2-down and selector-survival checks kept
  strict.

## Run: M3 CLOSURE (EFI 926d5568) — SELECTOR RESULT: transaction complete

FULL CHAIN CLOSED ON SILICON (AD107M, UEFI pre-driver window):
  scrubber  -> handoff 1->3 (domain un-gated)
  FWSEC     -> WPR2 verified (frts page exact)
  booter    -> MAILBOX0=0 (stock booter success; GSP-RM booted from our
               WprMeta/radix/sysmem publication)
  verdict   -> TX: execution verified
  FLR       -> GFW halted cores, WPR2 down (0x1ffffe00/0x0),
               selectors survive (AON island confirmed live)
  FINAL     -> transaction complete, all DMA released
  handoff   -> bootmgfw chainloaded clean, Windows resumed.

M3 acceptance met (task brief: full no-payload chain with read-back
evidence + Windows driver takeover). 15/15 chain steps walked with
read-back evidence. The unlock-relevant primitives (HS boot on both
engines, portal placement, BROM arming, FLR craft) are all proven.

## Run: M3 CLOSURE CONFIRMED (EFI 926d5568) — SELECTOR RESULT: transaction complete

Second consecutive full-chain run, now END TO END:
  scrubber handoff DONE -> FWSEC WPR2 verified -> booter MAILBOX0=0 ->
  TX: execution verified -> FLR (GFW halted cores, WPR2 down
  0x1ffffe00/0x0, selectors survive AON) -> FINAL: transaction complete,
  all DMA released -> clean bootmgfw chainload.
M3 acceptance (task brief): full no-payload chain with read-back
evidence + Windows takeover = MET on AD107M silicon.

## M4 prep (build dd1ed81b): payload signature topology fix

Caught by code read before the M4 go: payload mode patched the booter
IMAGE with the V67 blob - impossible by size (0xFA00=64000 > any booter
image; would EInval on AD102 AND GA102) and wrong by mechanism. The
exploit shape (unlock_610.c): the IMAGE always carries the STOCK
signature (the BROM validates the booter before it runs); the V67 blob
lives in the separate signature DMA buffer and WprMeta.sizeOfSignature
= 0xFA00 makes the booter's own unbounded signature copy overflow its
DMEM stack - that IS the ROP entry. Fixed: image always stock-patched;
V67 only in sig_dma/WprMeta; signature_restore (meta -> stock) before
FLR unchanged (constitution 9.5).

## Run: M4 CLOSURE (EFI dd1ed81b) — FOUR-WRITE UNLOCK EXECUTED AND VERIFIED ON 28e0

V67 payload mode, every gate green:
  booter fire    -> MAILBOX0 = 0x67b00001 (payload marker - ROP ran)
  FEAT_PLM       -> 0xffffff8f -> 0xffffffff
  WPRCFG         -> 0xfffff0ff (bits[11:8] left open per cmp170hx table)
  WPR            -> 0xffffffff
  FBPA           -> 0xffffff0f (observational, stuck as on 2705/2805/28e1)
  execution      -> TX: execution verified
  selectors      -> SS0 0x88888888 / SS1 0x8 written AND read back
                    (first working host writes through the opened PLM)
  signature      -> restored to stock before FLR (constitution 9.5)
  FLR            -> selectors SURVIVE (AON): ss0=0x88888888 ss1=0x8
  FINAL          -> transaction complete, all DMA released, Windows.

Expected Windows-side: FEAT_READOUT_1 0x00410080 -> 0 fields, ~2x
FP16/TF32-accumulate tensor issue rate (28e1 reference data). Unlock is
volatile: each cold boot replays via USB (project design).

M4 ACCEPTANCE MET. The nv-uefi-unlocker is now a functionally complete
Windows pre-driver unlocker: V67 four-write payload executed and
verified on RTX 4060 Laptop (10de:28e0), a card the community had NOT
verified (28e1 is the closest public canary).

## Windows-side acceptance (user-run, cli-stressor-rs)

FP16 GEMM 4096, resident L2 mode, same tool/machine:
  stock    : 29.89 TFLOPS(eqv)
  unlocked : 49.02-49.33 TFLOPS(eqv)  = 1.65x
1.65x < the 28e1 report's 2.00x because their figure is a pure tensor
issue-rate JIT microbenchmark; a real 4096 GEMM carries memory/
accumulation/epilogue components that dilute the ratio. Unlocked
absolute throughput (49.3 TF) is in the same band as 28e1's 52.97 TF.
Integrity self-tests green (pattern_clean 0 errors; injection_capture
exact). 1000 s stability run recommended as the full battery.

M4 ACCEPTANCE COMPLETE (firmware verdicts + Windows workload).

## Run: 28a1 silicon (RTX 4050 Laptop, EFI v0.1.1) 2026-10-26 — layout EIO

Same G614JV, card swapped to the 4050 (10de:28a1), machine left in hybrid
mode (iGPU primary, 2 GOP handles). Boot + chainload verified; the TX
itself died in layout with EIO, NOT a poison that poisons the whole card:

  SAMPLE VGA_WORKSPACE 0x625f04 = 0xbadf1301 [BADF_VARIANT]
  SAMPLE SEC2_CPUCTL 0x840100 = 0xbadf5620  (same bucket as 28e0)
  scrubber ok; GFW "stuck" accepted; layout -> EIO; FLR; clean chain.

ROOT CAUSE: layout_from_registers used strict read32(), which turns ANY
0xbadf_xxxx into EIO. VGA_WORKSPACE is the one layout input where poison
is tolerable - only bit3 feeds the decision and bit3=0 routes to the same
"reserve FB top 1 MiB" path the healthy 28e0 took.

STATE CORRELATION (supersedes the first "hybrid mode" reading): the SAME
machine in the SAME hybrid mode, when the dGPU owns the console (the
"hybrid shutdown" log below, 1 GOP handle), reads 0x625f04 normally,
reaches the bit3=0 path (vga = FB top 0x1ffb00000) and completes the
whole chain to selector read-back. The 4050 run had 2 GOP handles, i.e.
the iGPU was in the console. So the gate tracks "dGPU is not the display
owner", not hybrid power state. iGPU-only shutdown is the third state:
dGPU cold, no GOP at all, BAR0 all-ones.

FIX (next build): five raw reads with the raw values logged, poison
rejected strictly for FB/MMU PLM/lock, tolerated (fallback) only for
VGA; 0xbadf1301 recorded as a new 0x13xx bucket; claim preflight that
names D3cold when BAR0 stays all-ones after the MEM enable; sampler gets
a NO_DECODE bucket (0xffffffff used to be printed as "[ok]").

Expectation for the next run: layout passes; open risk is the GSP domain
(CPUCTL/HWCFG2/FBIF read-backs) - note that the successful 28e0 log above
shows those GSP regs equally gated (badf5620) BEFORE the engine reset and
clean after it, so gating per se is not the blocker.

## Control: 28e0 hybrid shutdown, dGPU as console (EFI v0.1.1, same machine)

User-supplied log, full success in hybrid mode. Distinctive state, in
order of appearance:

  GOP-PROBE: 1 GOP handle (the dGPU's)
  every SAMPLE read = 0xffffffff [ok]   <- BAR0 not decoding at handoff;
                                           the claim's MEM enable fixes it
  claim: raw COMMAND write cmd=0x0007 (MEM+BME+IO on)
  AUTH-SCAN: GSP CPUCTL=0x10 ok, but BOOTVEC/DMACTL/IMEM/DMEM = badf5620
  layout: fb=0x1ffc00000 vga=0x1ffb00000   <- VGA read FINE, bit3=0
  scrubber asset absent -> proceeded without
  FWSEC -> WPR2 verified; falcon reset ungated every badf5620 GSP reg
  booter -> MAILBOX0=0x67b00001; four writes -> selectors survive FLR

Two takeaways: (a) 0xffffffff at sample time is a decoder-off window,
not a card identity, and must not be printed as "ok"; (b) pre-reset
badf5620 on the GSP is the normal gated state and is reset away - the
open risk for the 4050 is not that gating exists.
