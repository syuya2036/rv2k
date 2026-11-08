# TCP シェル＋ENC の 16KB / 2KB 収容

シェル＋ENC が元の linker gate に合格しました。Flash は **16,382 / 16,384 byte**、静的 RAM 1,608 byte + kernel/IRQ/boot 256 byte = **1,864 / 2,048 byte** です。256 byte のタスクスタック 2 本は静的 RAM に含みます。Flash の余裕は 2 byte です。`scripts/shell.sh` は通常・IRQ 注入の検査後に `shell,drivers` を必ずリンクし、CI でも超過を拒否します。

前回の 444 byte 超過から、表示処理と TCP の共通処理を整理し、計測専用コードを実機用ネットワーク構成から分離しました。MIR / LLVM / linker フラグ、固定 nightly、memory.x、TCP buffer / MSS、quiet 60 秒・TIME_WAIT 120 秒、HTTP の絶対期限は変更していません。

|構成|Flash byte|全スタック込み RAM byte|
|---|---:|---:|
|shell 仮想 NIC・計測あり|15,048|1,908|
|shell＋ENC・計測なし|16,382|1,864|
|HTTP 仮想 NIC・計測あり|14,924|2,020|
|HTTP＋ENC・計測なし|16,018|1,952|
|snapshot probe・計測あり|14,312|1,932|
|TCP＋preempt＋ENC・計測なし|14,076|1,784|

## 変更と保持した処理

monitor の読める NUL 区切りソース表を、コンパイル時に長さ付きの表へ変換します。実行時の NUL 探索と表示ループ内の連続範囲管理を省き、共通 append で各文字を出力します。出力オブジェクトを 4 byte 境界へ揃え、u64 十進変換の保存 limb をまとめて検査します。公開 snapshot API、全状態名、符号付き終了コード、u64 全域、64 byte の出力上限を維持します。TCP は同じ処理だった終端状態をまとめ、接続終了で flight を消した後の不要な再検査を省きます。

`services::OBSERVE` は非ネットワーク probe または `emulator-stop` で有効です。実機用ネットワーク構成では raw の trap / IRQ / idle / switch / blocking-rejection カウンタ、最終 watermark の別名コピー、成功時 UART transcript、DRIVER_STATUS の検査記録を生成しません。arch の `test-observers` は延期回数だけを対象とし、`emulator-stop` または `preempt-timing` から有効にします。

preemption の depth / pending、借用中の切替禁止、Busy による不正待機の拒否、SysTick の acknowledge・clock 更新・切替、公開 task / mem / uptime / network snapshot、通信統計、FAULT_INFO と障害 UART、全番兵と SP の検査は常に残します。全終了時は計測なし構成でも全番兵を検査し、SERVICES_STATUS を記録して停止します。NIC の初期化・reset も常に実行します。

計測あり仮想 NIC と計測なし ENC は別 ELF です。仮想 NIC の高水位を実機の実測値として扱いません。実機での SPI タイミング、IRQ 下の通信と高水位は未検証です。

## 検査

workspace 88 tests、clippy、fmt が PASS です。標準 formatter との比較で u64 境界と 1,000 値、全 task / TCP 状態と符号付き終了コード、最大 snapshot を検査しています。

shell の各 run は 5 接続 / 138 入力 byte / 12 コマンドです。通常は 103 RX / 114 TX、強制切替 11 回、延期 IRQ 4 回、高水位 T0 / T1 / kernel = 176 / 216 / 120 byte でした。全 103 RX で IRQ を注入した run は 103 RX / 110 TX、延期 111 回、高水位 176 / 208 / 120 byte でした。全番兵は正常です。独立 PCAP checker が全コマンド、部分入力・異常入力後の復帰、再送・zero-window、EOF / RST 後の再接続と公開 snapshot を照合しました。

HTTP も通常・全 230 RX への IRQ 注入の両方で 17 接続 / 16 応答 / echo 本文 88 byte が PASS です。snapshot / TCP / ARP・IPv4・ICMP、compact helper、ドライバのホストモデル・MMIO、サービス、診断も PASS です。専用 preemption probe は 47 強制切替、SPI bit 3 内 IRQ、借用中の blocking syscall 拒否 8 回、全番兵を確認しました。復元欠陥 3 例、preempt profile の障害注入 9 例、RAM 境界成功と RAM / Flash 超過拒否も PASS です。

[source・ELF manifest / 実行ログ / 共通回帰](logs/shell-capacity)、[shell PCAP / SRAM / ENC budget](logs/shell)、[HTTP](logs/http)、[snapshot](logs/snapshot) に保存しています。旧フェーズのログは履歴として残し、今回の共通回帰は別ディレクトリに保存しました。

Mac の vmnet 起動は許可済みですが `sudo -n` が認証要求でネットワーク起動前に停止しました。Mac native ping / TCP / HTTP は未検証です。CH32V003J4M6＋ENC28J60 実機も未保有です。Phase 5〜7・9〜10 の外部検証条件を PASS にはしていません。
