# AD106 桌上型顯示卡（RTX 4060 Ti 16GB，10de:2803）：booter status 0x89 與上游 Root Port 的 Bus Master

2026-10-26，單機、單卡實測，Windows To Go 開機碟 + 本專案 `tx` 建置。

## 結果

GPU 上游的 Root Port（`8086:1901`），COMMAND 的 Bus Master（bit 2）是 0 的時候，回放與解鎖都以 `MAILBOX0=0x89` 收場。在 Booter 啟動前把這一位設起來之後：

- **回放**：`MAILBOX0=0`。
- **解鎖**：成功標記 `0x67b00001`，SS0/SS1 在 FLR 之後仍然保留。
- **GEMM 8192³ FP16**：46.78 → 75.28 TFLOPS。

在這台機器上，只改這一位，結果就從失敗變成成功（回放失敗與成功各跑一次，解鎖成功兩次）。

## 環境

| 項目 | 內容 |
| --- | --- |
| GPU | GeForce RTX 4060 Ti，`10de:2803`，16380 MiB，VBIOS 95.06.34.00.a4，165 W |
| 位置 | GPU 在 `0000:01:00.0`；上游 Root Port `8086:1901` 在 `0000:00:01.0`（pri/sec/sub = 0/1/1）；host bridge `8086:3e30`（Intel H310 平台） |
| 開機 | UEFI，Secure Boot 關閉，Windows To Go；`GOP-PROBE: 1 GOP handles`，由 `tx_detach` 中斷連線 |
| 驅動程式/基準 | Windows 驅動程式 616.92；torch 2.11.0+cu128 |
| 建置 | `cargo efi-tx`，Rust 1.99.1 |

## 失敗時的 log

`loader-replay-booter-0x89.log`（回放模式。這份 log 來自帶額外診斷輸出的建置，沒有放任何開關檔案，行為與上游相同）：

```
loader::tx_detach: DETACH: Attributes(Enable) no-op (cmd=0x0000) - raw COMMAND write
loader::tx_detach: DETACH: config baseline (vendor=0x10de dev=0x2803 cmd=0x0007 - MEM=true BME=true)
loader::pci: DMA-PATH[pre-fire]: gpu loc=Some((0, 1, 0, 0)) command=0x0007 status=0x0010 devctl=0x2930
loader::pci: DMA-PATH[pre-fire]: bridge 8086:1901 loc=Some((0, 0, 1, 0)) command=0x0003 status=0x0010 pri/sec/sub=0/1/1 bridgectl=0x0018
loader::tx_booter: POST-BOOT: SEC2 MAILBOX0=0x00000089 WPR2_HI=0x3ffae00
loader::tx_booter: POST-BOOT: booter status 0x89 (nonzero = failed)
loader::pci: DMA-PATH[post-fail]: gpu loc=Some((0, 1, 0, 0)) command=0x0007 status=0x2010 devctl=0x2930
```

- Root Port `8086:1901` 的 command 是 `0x0003`，bit 2 是 0；PCH 那一側的其他橋接器是 `0x0007`。
- GPU 的 PCI status 在 Booter 執行之後從 `0x0010` 變成 `0x2010`（bit 13，Received Master Abort）。
- 故意把 WprMeta 的 magic 改壞、或切到解鎖模式，狀態碼仍然是 `0x89`。

## 修法

`loader/src/pci.rs` 新增 `BridgeMaster`：

- 對每個「與 GPU 同一個 segment、secondary..subordinate 範圍涵蓋 GPU 所在匯流排」的 PCI-to-PCI 橋接器，如果 COMMAND 的 bit 2 是 0，就把它設起來並讀回確認。
- 離開時（`Drop`）只把這一位清回去，`run_transaction` 中途出錯而提前返回時也一樣會還原。
- `tx.rs` 在 `booter_fire` 之前建立它，一直保留到 `run_transaction` 返回為止。
- 另外有唯讀的 `log_dma_path()`，在前後各記錄一次 GPU 與所有橋接器的 COMMAND/STATUS/匯流排編號。

**不改動**：GPU 暫存器的操作順序、資產檔案、Booter 的載入方式、解鎖的判定條件。

## 修好之後

`loader-replay-bridge-bme.log`（回放模式）：

```
loader::pci: BRIDGE-BME: Some((0, 0, 1, 0)) command 0x0003 -> 0x0007
loader::tx_booter: POST-BOOT: SEC2 MAILBOX0=0x00000000 WPR2_HI=0x3ffae00
loader::tx: TX: execution verified
loader::tx: POST-FLR: verified (selectors survive AON)
loader::tx: FINAL: transaction complete, all DMA released
loader::pci: BRIDGE-BME: restored Some((0, 0, 1, 0)) command 0x0007 -> 0x0003
```

`loader-unlock-bridge-bme.log`（解鎖模式，有 V67；PR 建置加上 segment 比對之前的版本）：

```
loader::tx_booter: POST-BOOT: FEAT_PLM=0xffffffff SS0=0x46512002 SS1=0x0
loader::tx_booter: POST-BOOT: SEC2 MAILBOX0=0x67b00001 WPR2_HI=0x3ffae00
loader::tx_booter: POST-BOOT: WPRCFG=0xfffff0ff FBPA=0xffffff0f WPR=0xffffffff
loader::tx: SELECTOR RESULT: SS0=0x88888888 SS1=0x8
loader::tx_booter: POST-FLR selectors: ss0=0x88888888 ss1=0x8
loader::tx: POST-FLR: verified (selectors survive AON)
loader::tx: FINAL: transaction complete, all DMA released
loader::pci: BRIDGE-BME: restored Some((0, 0, 1, 0)) command 0x0007 -> 0x0003
```

`loader-unlock-final-build.log`（解鎖模式，有 V67；這個 PR 最終版程式碼的建置）：

```
loader::pci: BRIDGE-BME: Some((0, 0, 1, 0)) command 0x0003 -> 0x0007
loader::tx_booter: POST-BOOT: FEAT_PLM=0xffffffff SS0=0x42512002 SS1=0x0
loader::tx_booter: POST-BOOT: SEC2 MAILBOX0=0x67b00001 WPR2_HI=0x3ffae00
loader::tx: SELECTOR RESULT: SS0=0x88888888 SS1=0x8
loader::tx_booter: POST-FLR selectors: ss0=0x88888888 ss1=0x8
loader::tx: POST-FLR: verified (selectors survive AON)
loader::tx: FINAL: transaction complete, all DMA released
loader::pci: BRIDGE-BME: restored Some((0, 0, 1, 0)) command 0x0007 -> 0x0003
```

`loader-unlock-bridge-bme.log` 那次開機進入 Windows 後複核（165 W，單位 TFLOPS）：

| 類型 | N=4096 前 → 後 | N=8192 前 → 後 |
| --- | --- | --- |
| FP32（對照組） | 12.16 → 12.03 | 12.17 → 12.04 |
| FP16 | 45.12 → **68.87** | 46.78 → **75.28** |
| BF16 | 45.74 → **68.11** | 47.24 → **76.71** |

`correctness=OK` 全部通過。「前」是同一台機器未解鎖（原廠冷態）時的測量值，「後」是 `loader-unlock-bridge-bme.log` 那次開機進入 Windows 之後的測量值。

## 說明

- 失敗的回放與修好之後的回放，這兩份 log 來自帶額外診斷輸出的除錯建置，`BRIDGE-BME` 的行為與 PR 建置相同；兩份解鎖 log 是 PR 建置跑的，`loader-unlock-final-build.log` 來自最終版程式碼。PR 建置裡 log 的標籤是 `before-bridge-bme`/`after-bridge-bme`。log 維持原始的 CRLF 行尾。
- 只有一台機器、一張卡。長時間負載下的穩定度、溫度與功耗，都沒有測。
- `cargo efi` 與 `cargo efi-tx` 都是零警告；`cargo xt` 全部通過（`assets` 19 項、`sim` 44 項）。`sim/` 沒有模擬橋接器，所以沒有擴充。
