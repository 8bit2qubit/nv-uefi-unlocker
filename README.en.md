# nv-uefi-unlocker

[中文](README.md) | [English](README.en.md)

A UEFI pre-driver NVIDIA GPU unlocker (Windows, Rust). It performs the
complete firmware transaction in the UEFI DXE window — **before** the NVIDIA
driver loads: SEC2 scrubber → FWSEC-FRTS (WPR2) → signed Booter booting
GSP-RM → (gated) V67 four-write payload → read-back verification → FLR →
clean handoff to the Windows Boot Manager.

**Status: M4 complete.** Executed and verified end-to-end on an RTX 4060
Laptop (`10de:28e0`, AD107M) — firmware verdicts (payload marker
`0x67b00001`, FEAT PLM `0xffffffff`, AON selector survival across FLR) and
Windows-side FP16 GEMM 29.89 → 49.33 TFLOPS (1.65×). Full report:
[docs/ada28e0-uefi-report.md](docs/ada28e0-uefi-report.md).

This card had never been verified by the community (closest public canary:
28e1, same chip, also an ASUS laptop).

## Usage (RTX 40 series, AD10x family)

### USB stick root layout (FAT32; `make-usbroot.ps1` produces this)

| File | Purpose | Required |
| --- | --- | --- |
| `BOOTX64.EFI` | This tool (back up any existing file of that name) | ✅ |
| `booter_load_ad102.bin` | AD102-family Booter bundle (BTL1) | ✅ |
| `gsp_boot_ad102.bin` + `gsp_boot_desc_ad102.bin` | GSP-RM boot segment + v5 descriptor | ✅ |
| `fwimage_ad102.bin` | GSP-RM firmware image (~80 MiB, radix3) | ✅ |
| `fwsignature_ad102.bin` | GSP-RM stock signature (referenced by WprMeta) | ✅ |
| `scrubber_ad10x.bin` | Kernel-identical SEC2 scrubber (BSI handoff 1→3) | ✅ |
| `v67-ad102-four-plm.bin` | **The V67 four-write payload — its presence = unlock mode** | optional |
| `\loader.log` | (output) full stage log, the evidence record | output |

Without `v67-*.bin` the tool runs in **replay/diagnostic mode**: the full
chain with zero unlock writes.

### Procedure

1. BIOS: disable Secure Boot; set the display mode to dGPU-only
   (Ultimate/dedicated) — in hybrid/Optimus mode the EC puts the dGPU into
   D3cold early in boot and the BAR0 VBIOS aperture becomes invisible.
2. Boot from the stick → banner → **press Space to confirm** (auto-returns
   to Windows after 30 s).
3. Everything after that is automatic; the tool chains back into
   bootmgfw whether the transaction succeeds or fails. The full log lands
   in `\loader.log` on the stick. The lookup order is fixed: **bootmgfw →
   distro shim/grub/systemd-boot → the stick's generic
   `\EFI\BOOT\BOOTX64.EFI`**. Only that last tier — which is normally this
   very application — opens a 5-second window: any key returns to the
   firmware shell (the unlock is already done and logged, and any OS can be
   started from there).
4. Re-run once per cold boot — the unlock is **volatile** (nothing
   persistent, no flashing, cleared by power loss).

Regenerate the payload and all assets:
`cargo run -p assets -- generate-ad102` / `release/make-usbroot.ps1`
(reproducible from pinned SHA-256 610.43.03 public inputs, hard-verified).

## Combining with the nvoc kmd power-wall unlock

This tool is **compatible and orthogonal** with
**[Skyworks-Neo/nvoc](https://github.com/Skyworks-Neo/nvoc)**: it removes
the tensor issue-rate clamp in the UEFI window (FEAT/WPR/WPRCFG PLM), while
nvoc's kmd removes the power/voltage wall inside the OS. They stack.

### Measured extremes (RTX 4060 Laptop 28e0, FP16 GEMM + nvoc)

> All FP-precision throughput figures in this document were measured with
> [cli-stressor-cuda-rs](https://github.com/Skyworks-Neo/nvoc/tree/main/cli-stressor-cuda-rs)
> from [Skyworks-Neo/nvoc](https://github.com/Skyworks-Neo/nvoc).

| State | Value |
| --- | --- |
| FP16 GEMM 4096 (this tool only) | 49.0–49.3 TFLOPS (stock 29.89, 1.65×) |
| FP16 GEMM 8192 (+nvoc power wall) | **60.8–63.1 TFLOPS** (~**2.1×** vs stock) |
| Core clock peak | **3045 MHz** @ 1.000 V |
| Board power peak | **206.8 W** (chip ~163.7 W + VRAM 21.8 W + PWR_SRC 24.8 W; 16-pin 164.4 W @ 19.8 V) |
| Temperatures | Hot Spot peak **101.8–104.4 °C**, core 79–81 °C, VRAM 58–66 °C |
| Limiter | PerCap Reason alternates between **Pwr** and **Thrm** |

Interpretation: at full load the card first hits the power wall (Pwr); with
the wall lifted it hits the thermal wall (Thrm) — **3045 MHz / 206.8 W /
~104 °C Hot Spot is the physical limit band of this 8 GB 4060 Laptop**.

### What the unlock actually is: disabled transistors back to work, not overclocking

The clamped SM issue rate is a **periodically-stalled tensor datapath**
(manufacturing segmentation: the cut lanes spin on NOP/stall slots in the
instruction stream while the clock keeps ticking) — unrelated to clocks,
voltage or power walls.
Controlled experiment (RTX 4060 Laptop, ASUS ROG Strix G16 **G614JV**,
FP16 tensor, same card and precision, the only variables being the issue
rate and the power wall):

| State | Clock | Chip power | Wall hit | FP16 tensor |
| --- | --- | --- | --- | --- |
| stock (power wall lifted) | **3.0 GHz** | 155–160 W | **no power wall hit** | 35.9 TFLOPS |
| SM issue-rate unlocked (+nvoc) | avg **2.89 GHz** | 190–208 W | thermal + power-delivery limit | **64.30 TFLOPS** |

After the unlock the frequency **drops 3.7%** and power rises 34%, yet
throughput rises **79%** — the extra FLOPs cannot come from clocks or
power; they come from **the full transistor set no longer idling**: with
the segmentation mask lifted, the stall slots that consumed NOPs every
clock edge load real instructions again and the participating tensor-core
count returns to the full design set — the power increase is the direct
consequence of idling becoming work. This is why
overclocking/over-power alone can never close the gap (stock at 3.0 GHz
hitting no wall only reaches 35.9 T) while the unlock more than doubles
throughput at a *lower* clock.

> **⚠️ Hardware risk warning**
> Stacking both unlocks pushes power and heat density **far outside the
> factory envelope** (measured: 200+ W board power, Hot Spot around
> 105 °C sustained). You must verify and continuously monitor:
> - **Cooling**: paste/LM condition, fan curves, vapor-chamber capacity —
>   a >20 °C Hot-Spot-to-core delta means the interface is at its limit;
> - **Power**: adapter rating, VRM headroom, 16-pin link temperatures;
> - **Monitoring**: keep Board Power, Hot Spot and PerCap Reason on screen.
>
> **No liability is accepted for physical damage to the chip, power
> delivery or system caused by over-power/over-voltage/over-temperature.**
> The unlock is volatile — a fried die does not recover.

## How it works (overview)

The compute clamp is a hardware gate on the PRI bus (PLM, default
`0xffffff8f` = writable only by heavy-secure firmware); host/driver writes
to protected registers are silently dropped. The only path through it is to
have **signed firmware itself** write from the privileged side. This tool
replicates the kernel driver's boot chain in the UEFI window and injects
the community-reversed V67 payload at the "hand the Booter its signature"
step:

```
claim(detach GOP/drivers + exclusive) → scrubber(BSI handoff 1→3)
→ FWSEC-FRTS(WPR2) → booter(stock signature, BROM-validated, mailbox=WprMeta)
→ [V67: unbounded signature DMA overflows the DMEM stack → ROP → four
writes FEAT/WPR/WPRCFG/FBPA + marker 0x67b00001]
→ selector writes (host writes stick once the PLM is open)
→ signature restore → FLR(cold-card handback) → bootmgfw
```

Every step is read-back verified; on any failure the tool refuses and FLRs.
Nothing persists, nothing is flashed.

## Compatibility matrix

Routing is per **chip family** (all IDs of a family share the same
booter/payload assets, matching the upstream `ad102_family()` routing).
Tiers: ✅ verified here; 🟢 upstream silicon-verified (same family proven in
the kmod, our EFI path is structurally identical — expected compatible);
🟡 path exists, no silicon evidence; ❌ incompatible.

| Family | Cards (device IDs) | Tier |
| --- | --- | --- |
| AD107 | 4060 Laptop **28e0** | ✅ verified (this repo) |
| AD107 | 4060 2882 / 4060 Laptop 28a0 / 4050M 28e1 | 🟢 upstream-verified |
| AD106 | 4060Ti 2803 / 4060Ti 16G 2805 | 🟢 |
| AD104 | 4070Ti 2782 / 4070S 2783 / 4070 2786 | 🟢 |
| AD103 | 4080S 2702 / 4080 2704 / 4070TiS 2705 / 4070 2709 / 4090M 2757 | 🟢 |
| AD102 | 4090 2684 / 4090D 2685 / 4070TiS 2689 | 🟢 |
| GA10x | 3090/3080 2204/2206, 3070s 2484–248c, 3060s 2501/2503–2509/252f/2544, 3050 2584, 2560, 20c2/20f1 | 🟡 kmod-verified upstream; our EFI path has no silicon evidence |
| Turing & older | 20/16 series, Pascal… | ❌ no transaction track (Turing assets staged but disabled; Turing has no known clamp) |
| Blackwell (50 series) / GA100 | — | ❌ different chain, out of scope |

**"Does this work on all RTX 40 cards?" — every AD10x (40-series) ID is
listed above**: same-family assets plus upstream per-silicon verification
make compatibility highly likely, but only 28e0 has been verified through
this EFI. For first use run **replay mode** (no `v67-*.bin` on the stick):
reaching `TX: execution verified` means the whole path works; add the
payload afterwards.

**If something does not work**: open an
[Issue](https://github.com/physicsdolphin/nv-uefi-unlocker/issues) with the
full `\loader.log` from the stick (the log is the only diagnostic input;
feel free to strip machine-specific details first). Success/failure data
points (card model + key log lines + verdict values) are welcome in
[Discussions](https://github.com/physicsdolphin/nv-uefi-unlocker/discussions)
to extend the matrix.

> The maintainer works on this **part-time** and may not see or answer
> issues immediately — logs are kept and will be reviewed. Patience
> appreciated.

## Repository layout

```
loader/    EFI application crate (no_std, x86_64-unknown-uefi, Rust + uefi-rs)
assets/    host-side asset pipeline (pinned-hash download/verify/extract/generate)
sim/       behavioral simulator + transaction state-machine tests (no GPU)
docs/      reports, milestone evidence, register cold-value tables, open questions
```

## Safety constitution (copy, equivalent to AGENTS.md §1)

1. **Read-back is the only success criterion.** Status codes, return
   values, interrupts and self-test output never count as proof.
2. **Never retry in the same session after a STOP/fault**; power off
   completely and resume through normal Windows boot.
3. Chain-load the ESP's `\EFI\Microsoft\Boot\bootmgfw.efi` **read-only**;
   never overwrite anything on the ESP; back up the stick's BOOTX64.EFI
   before replacing it.
4. No persistent boot entries: one-shot from USB/boot menu only; never
   touch NVRAM BootOrder.
5. Pin every card's identity (PCI vendor:device + ROM hash); refuse on
   mismatch.
6. Never distribute or commit NVIDIA firmware; regenerate locally from
   pinned SHA-256 public inputs; keys/decrypted artifacts/payloads never
   enter the repository.
7. Gated writes: zero writes before M1; explicit user confirmation before
   M3; M4/M5 are separate projects.
8. Mock before hardware: transaction code must be fully green in the
   behavioral simulator first.
9. Secure Boot disabled or a self-signed cert in db (preferred).
10. License: GPL-2.0-only.

## Rollback manual

### A: black screen (chainload failure / mid-transaction)
No firmware configuration or ESP file was touched → force reboot → boot
Windows from the original boot entry. The stick is not in the boot chain
unless you boot from it.

### B: STOP/fault mid-transaction
**Never retry in the same session** → power off completely (laptops:
discharge the EC, e.g. ASUS = unplug + hold power 40 s) → remove the stick
→ boot Windows normally → check Device Manager / event log → archive the
evidence before the next attempt.

### C: Windows driver misbehaves after handoff
Check Device Manager error codes and nvlddmkm/Xid events. The unlock is a
volatile overlay (gone after power loss) — one cold boot returns the card
to stock. If issues persist: another full power cycle; last resort DDU +
driver reinstall (the tool writes nothing persistent; a misbehaving driver
can only come from handoff timing, not permanent damage).

## Build

```
cargo build --release   # host crates (assets/sim); loader is not in default-members
cargo efi               # read-only EFI build (M0/M1 diagnostics)
cargo efi-tx            # gated transaction build (write paths; explicit use only)
cargo xt                # all host tests (assets 19 + sim 44)
```

A bare `cargo build --workspace` tries to build the loader for the host
triple and fails — the loader only builds for `x86_64-unknown-uefi`; use
the aliases above.

## Community contributions and references

Full credits in
[docs/ada28e0-uefi-report.md](docs/ada28e0-uefi-report.md). Core:

- [gpu-unlock](https://github.com/Zagitta/gpu-unlock) (GPL-2.0) — the
  transaction-chain porting source
- [gpu-unlock-ada](https://github.com/JustAResearcher/gpu-unlock-ada)
  (GPL-2.0) — the AD10x four-write chain; the 28e1 report (Windroid234) is
  this project's direct basis
- [cmp170hx](https://github.com/Consensus-Protocol/cmp170hx) — PLM
  semantics and verification methodology
- [cmp90hx-unlock](https://github.com/WildFlash1st/cmp90hx-unlock) — the
  V67 chain origin
- [AD103_win_unlocker](https://huggingface.co/TOTORONG/AD103_win_unlocker)
  (TOTORONG) — the Windows EFI carrier-craft precedent
- [open-gpu-kernel-modules](https://github.com/NVIDIA/open-gpu-kernel-modules)
  (MIT) — authoritative boot-path sources

License: **GPL-2.0-only** (`LICENSE`). NVIDIA firmware and VBIOS remain
NVIDIA property; regenerate them locally from pinned public packages under
their own license terms — they are not distributed here.
