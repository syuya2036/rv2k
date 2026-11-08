# CH32V003 Rust / macOS 実動検証

Apple SiliconのmacOS上で、RenvoのネイティブビルドとRust RV32ECプログラムの実行を確認しました。これはOSカーネルの開発基盤の実証です。タスクスケジューラやシェルの完成を意味しません。詳細は [検証レポート](docs/verification.md)。

- 基本プログラム: 174命令、`RESULT=0x123456a2`、USART1に `Rust RV32EC OK\n`。
- SysTick版: 304命令、割り込みハンドラで `TICKS=1`、`mret` 復帰後UART送信。
- ELF: RV32E + C、ILP32E、ヒープなし、Flash 16KB / RAM 2KB。
- Renvoの `.data` LMAロードに回避処理が必要。元の実機用ELFとエミュレータ用 `.remu` ELFを別々に生成。

## 協調2タスク

2タスク固定、各256バイトの独立スタック、ヒープなしの協調切り替えを追加しました。
セットアップ完了後に実行できます。

```bash
./scripts/coop.sh
python3 scripts/coop-negative.py
```

A/B各8回更新、32回のスイッチ、全RV32Eレジスタの保存復元検証、スタック使用量各172バイトを確認しました。静的RAM592バイト（タスクスタックを含む）＋起動スタック予約512バイト、Flash text1553バイトです。詳細と制約は [協調マルチタスク検証](docs/cooperative.md)。

## 前提と環境への影響

Git、Rustup、macOSのCリンカ（Xcode Command Line Tools）、Python 3が必要です。未導入の管理者権限が必要な操作は、この検証では実施していません。Rustupは既存環境にありました。

下記はRustup管理下に固定nightlyと `rust-src`、LLVMツールを追加します（数百MB以上）。既定ツールチェーンは変更しません。Cargoの依存キャッシュは通常のユーザーディレクトリに保存します。ソース、ビルド出力はこのディレクトリ配下です。Dockerは不要です。

## コピーして再現

このディレクトリを含むプロジェクトのルートで実行します。

```bash
cd ch32v003-rust-os-test
uname -a
uname -m
sw_vers
rustc --version
cargo --version
rustup show
rustup toolchain install nightly-2026-03-12 --profile minimal --component rust-src --component llvm-tools-preview
./scripts/setup-emulator.sh
./scripts/build.sh
./scripts/run.sh
./scripts/timer.sh
# 通常版へ戻し、境界・GDBプロトコル検証を実行
./scripts/build.sh
./scripts/run.sh
python3 scripts/probes.py
```

期待する出力:

```text
PASS: 174 instructions; RESULT=0x123456a2; UART=Rust RV32EC OK
PASS: SysTick handler wrote TICKS=1 and returned; instructions= 304
PASS: RAM/Flash overflow rejected; GDB RSP registers, memory, step, detach
```

`setup-emulator.sh` はRenvoを固定コミットでcloneして `cargo +stable build --release --locked -p remu-cli` 相当のビルドを行います。ファームウェアのCargo設定をホストツールへ適用しないよう、一時ディレクトリから実行します。初回はクレートのダウンロードとコンパイルに時間がかかります。

## 出力とデバッグ

- 実機向けELF: `target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test`
- Renvo向けELF: 上記に `.remu` を付加
- ログ: `docs/logs/run.json`, `bus.json`, `disassembly.txt`, `elf-layout.txt`
- サイズ: `docs/logs/size.txt`。ELFファイル全体のサイズはデバッグ情報を含むためFlash使用量ではありません。

```bash
./vendor/renvo_emu/target/release/remu targets
./vendor/renvo_emu/target/release/remu inspect target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test.remu
./vendor/renvo_emu/target/release/remu gdb \
  --target ch32v003 \
  --elf target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test.remu \
  --listen 127.0.0.1:3333 --artifact target/gdb-session.json
```

別端末のRISC-V対応GDBで `target remote :3333` を利用する構成です。今回実行したのは付属PythonによるGDB RSP検証であり、GDBクライアント自体は未検証です。

実機では `ebreak` はプロセス終了を意味しません。UARTのピン、クロック、PFIC/HPE設定、書き込み・デバッグ経路は実機購入後に確認してください。タイマー版はRenvoの直接トラップ経路を検証するもので、WCH公式のHPE有効・mode 3ベクタ起動と同一ではありません。

## Phase 6: virtual Ethernet / ARP / IPv4 / ICMP

`bash scripts/network.sh` でフレーム注入・guest番兵・仮想NIC/ENC構成の容量を検査します。
Mac実pingは未検証のためPARTIALです。[試験範囲と再現手順](docs/network.md) を参照してください。

## Phase 10 TCP シェル

`bash scripts/shell.sh` は Renvo 上の実 RV32EC 命令で port 23 の ps / mem / uptime / netstat、入力エラー復帰、未完了 EOF / RST、再接続を通常・RX 中 IRQ 注入の両方で検査します。元 ELF は Flash 16,320 byte、予約 RAM 1,964 byte、全番兵が正常です。ENC 版は専有 SPI デバイスへの変更後も Flash 1,616 byte 超過、Mac native TCP と実機は未検証です。[仕様・結果・残作業](docs/shell.md) を参照してください。
