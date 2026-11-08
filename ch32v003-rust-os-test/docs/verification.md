# CH32V003 Rust OS 開発基盤の検証

追加検証: [協調マルチタスク](cooperative.md) で2タスク固定・各256バイトのスタック・32回の切り替えを実証。以下の起動/UART/割り込みに関する初回レポートに加え、協調タスク切り替えはPASSとなった。プリエンプションは未検証。

検証日: 2026-11-08。PASSはこのホストで実行成功、FAILは試行失敗、PARTIALは限定された成功、UNVERIFIEDは未実行。ソースからの判断は実行結果と区別する。

## A. 結論

Apple Silicon Macだけで、RV32EC / ILP32EのRust no_stdプログラムを生成し、CH32V003の16KB Flash・2KB RAMマップ内でRenvo実行する開発基盤は成立した。Docker不要。ただしRenvoのELF `.data` LMA処理に回避策が必要で、実機の割り込み・クロック・ピン・実時間忠実性まで保証できない。Intel Macは実行未検証。

タスク管理やメモリ保護のある完成OSではなく、起動、RAM、UART、SysTick割り込みの最小実証である。

## B/C. 動作結果・対応状況

|検証項目|結果|根拠・範囲|
|---|---|---|
|macOSネイティブビルド|PASS|`remu-build.log`、`setup-emulator.log`。Mach-O arm64|
|Apple Silicon動作|PASS|macOS 26.6.2 arm64でCLIとゲストを実行|
|Intel Mac動作|UNVERIFIED|x86_64 Macなし。移植可能そうなRust実装であることは実証ではない|
|remu-cli / ch32v003の存在|PASS|Cargo.toml、`targets.txt`、実行ログ|
|stable 1.92.0のみで通常のcore付きRV32ECビルド|FAIL|RV32Eターゲット名はあるが配布coreなし。`stable-target.log`, `stable-build.log`|
|nightly RV32ECコンパイル|PASS|nightly-2026-03-12、LLVM 22.1.0、core/compiler_builtinsを再構築|
|ELF生成|PASS|RVE/RVCフラグ、ILP32E、`rv32e2p0_c2p0_zca1p0`。`elf-layout.txt`|
|元のELFの.data初期化|FAIL|起動時Flash LMAから0xffffffffをコピーしRESULT=41。`bus-before-lma-fix.json`|
|回避後のエミュレータ実行|PASS|174命令、Halted、RAM=0x123456a2、UART一致。`assertions.log`, `run.json`, `bus.json`|
|SRAM 2KB / Flash 16KB制限|PASS|2049 / 16385 byteのPT_LOADを拒否。境界0x20000800 / 0x4000でUnmapped fault|
|スタック含むRAM予算|PARTIAL|リンク時512 byteを予約し重複ASSERT。動的最大スタック量やタスク別ガードは未検証|
|UART TX|PASS|実USART1 MMIO経由の15 byte出力を照合。エミュレータ専用UARTは使わない|
|UART RX / 対話シェル|UNVERIFIED|RXキューAPIはソースにあるが、端末からの対話入力経路は未実装・未実行|
|GPIO|PARTIAL|GPIOポート実装、TX用設定、上流テスト。J4M6の物理ピン数制約までは再現しない|
|SysTick|PASS|比較値100、ハンドラがTICKS=1を書き込み復帰。`timer-bus.json`|
|割り込み|PARTIAL|直接mtvec、mstatus.MIE、SysTick、mretは実行。mode 3は上流CPUテスト成功。厳密PFIC/HPEは近似|
|GDB|PARTIAL|RSPでレジスタ・メモリ読取、step、detach成功。実GDB接続は未検証|
|タスク切り替え|PASS|追加検証で協調2タスク・32回切替・レジスタと独立スタック保持を確認。[詳細](cooperative.md)|
|Docker実行|UNVERIFIED|`docker info`成功のみ。ネイティブ成功のためコンテナ内ビルド・実行は未実施|
|実機実行|UNVERIFIED|CH32V003J4M6を接続していない|

通常版 `llvm-size`: text=225, data=4, bss=4 byte。Flash上のdata配置はアラインメントを含むため実ロード範囲は232 byte。RAM静的領域8 byte＋予約スタック512 byte。タイマー版はtext=317, data=4, bss=8 byte。ヒープ・アロケータなし。ソフトウェアが48MHzで動作する実時間測定ではない。

## 検証環境・固定ソース

[環境ログ](logs/environment.txt): Darwin 25.6.0、macOS 26.6.2、arm64。ホストstable rustc/cargo 1.92.0、ゲストnightly rustc 1.96.0-nightly (3b1b0ef4d, 2026-03-11)、LLVM 22.1.0。

- Renvo: `6b0c7221ca32e83903797d2c86a8effdcd62550a`
- ch32-hal: `f6b5c38a2399503e8d3984b4b51a2da3adc79b30`
- ch32v00x-hal: `e06f58bb57a93e69df728b9146fd97d8114fbca0`
- Renode platforms: `90999f0a28ba75b65662fc2d300d4bd32e104d69`

取得ソースは `vendor/` に保存しGit管理から除外。RenvoとHALはいずれもcloneした。HALを依存としてビルドしたわけではない。元WCHのCoreヘッダとStartupは `docs/wch-core_riscv.h`, `docs/wch-startup.S` に取得した（末尾の公式URL参照）。

## D. 実行コマンドと再現

[README](../README.md) に最初からのコマンドを掲載。最初に実際に実行したコマンド:

```bash
uname -a
uname -m
sw_vers
rustc --version
cargo --version
rustup show
git clone https://github.com/tinyrange/renvo_emu.git vendor/renvo_emu
cd vendor/renvo_emu
cargo build --release -p remu-cli
./target/release/remu targets
```

ビルドの終了コード0を確認し、実バイナリでtargetsを実行した（`targets.txt`）。後から親ディレクトリにファームウェア用 `.cargo/config.toml` を追加したため、再現は `setup-emulator.sh` を使用する。CLIの現在のコマンドは要求通り有効である。

```bash
# ファームウェア側
cargo build --release --locked
./scripts/build.sh
./scripts/run.sh
./scripts/timer.sh
./scripts/build.sh
python3 scripts/probes.py
```

`build.sh` はLLVMによるISA属性、逆アセンブル、サイズ出力、ELF変換も行う。`run.sh` は単に起動するだけでなく期待するUART、RAM書込み、停止理由、命令数をassertする。`timer.sh` もハンドラによる書込みをassertする。

上流テスト（ホスト対象を明示）:

```bash
cd vendor/renvo_emu
cargo +stable test --release --target aarch64-apple-darwin -p remu-cpu-riscv
cargo +stable test --release --target aarch64-apple-darwin -p remu-devices wch
cargo +stable test --release --target aarch64-apple-darwin -p remu-machines wch
```

CPU 28件、WCH device 40件、WCH machine 14件成功。全アーキテクチャの全テストを実行したわけではない。Intelでは上記ターゲットを `x86_64-apple-darwin` に変更するが、今回その実行はしていない。

### 失敗と回避策

1. `rustup target add --toolchain stable riscv32e-unknown-none-elf`: target not supported。`rustc --print target-list`に存在しても標準ライブラリ配布を意味しない。stable `cargo build --target riscv32e-unknown-none-elf` もcoreなしのE0463。公式ページの「rustupでインストールできる」という現行記載と、このインストール済みバージョンの実測は異なる。
2. 最初のカスタムJSONビルド: `.json target specs require -Zjson-target-spec`。`.cargo/config.toml` に `[unstable] json-target-spec=true` を追加して解決。
3. 元のELF: RAMのSEEDにはローダが正しい値を入れるが、Flash側LMAには未配置。起動処理がFlashから誤値をコピーした。`remu-image/src/lib.rs` はp_paddrを `load_address` に保持するが、`remu-machines/src/riscv/image.rs` のloadループは `segment.address` のみに書く。`elf-for-remu.py` は初期化済みPT_LOADについてVMAとLMAが違う場合、Flash LMAにも同じデータをロードするPT_LOADを追加する。実機ELFの命令とリンカスクリプトは維持。Renvo自体は改変していない。
4. ELF変換をllvm-objcopyのsectionアドレス変更で試したが、非再配置ELFではエラー。またsection追加だけではPT_LOADを使うローダに反映されなかった。最終的にPT_LOADを追加する方式に変更。
5. 親のCargo設定がvendor内のホストテストにも適用され、組み込みターゲットにstdがないエラー。ホストターゲットを明示して再実行した。
6. 固定nightlyインストールの同時呼出しでRustupの一時ダウンロードファイルエラーが発生。既存nightlyで検証を進め、固定nightlyのインストール完了後、通常のスクリプトを再実行して成功した。再現はRustupインストールを先に単独で完了させる。

## E. Renvoの実装・技術的制約

固定コミットの [ソース](https://github.com/tinyrange/renvo_emu/tree/6b0c7221ca32e83903797d2c86a8effdcd62550a) を参照。

- workspace Cargo.toml: edition 2024、rust-version 1.87、`remu-cli` がworkspace member。CLI Cargo.tomlのbin名は `remu`。OS固有のGUIを必要としないheadless構成。arm64ネイティブ実行を確認したがIntel互換の証拠にはしない。
- `remu-cpu-riscv/src/core.rs` のch32v003プロファイル: レジスタ16本、C/Zicsr/XW有効、M/A/B無効、user_mode=false、QingKe割り込み、reset PC=0。RV32Eのx16以上の拒否、C命令、8種のWCH圧縮メモリ命令を上流テストで確認。全命令・全組合せの適合試験ではない。今回のRustは独自XWを有効にしない。
- `remu-machines/src/target.rs`: Flash 0..0x3fff、SRAM 0x20000000..0x200007ff。`riscv/wch_flash.rs` に0x08000000 Flash別名と0x40022000 Flashコントローラがある。実際のバス境界を負例で確認。
- `riscv.rs`: GPIOA=0x40010800、GPIOC=0x40011000、GPIOD=0x40011400、USART1=0x40013800、RCC=0x40021000、PFIC=0xe000e000。PFICウィンドウ内SysTick=0xe000f000。GPIOモデルは18本級ターゲットでSOP8の6 GPIOだけへの制限はない。
- `remu-devices/src/wch.rs` と `riscv/systick.rs`: 32bit SysTick、HCLKまたは/8、比較・割り込み12をモデル化。1命令アクションを仮想tickとして扱い、Flash wait state、48MHzクロック、UART波形の時間精度は未保証。STREクリア時の独自up/down動作はソースコメントでも近似扱い。
- `remu-gdb` とCLIの `GdbArgs`: TCP RSP、ブレーク/ウォッチなどのデバッグ基盤がある。今回検証した具体的操作は `gdb-rsp.json` に保存。
- `--interrupt-log` は今回のSysTick版で空配列だった。これを割り込み不発と解釈せず、ハンドラPCの命令、TICKSへの実書込み、復帰後処理を証拠にした。全割り込み源のログ網羅性は未保証。
- `ebreak` によるHaltedを正常な期待停止と定義した。`exit_code=null` であり、実機でプロセス終了コードを返したわけではない。エミュレータのテスト専用GPIO/UART/exit領域は今回使用しない。

### Rust / ABI

Rust stableはLLVMにRV32E対応を持つが、今回の通常のcore付き開発にはnightlyのbuild-stdが必要だった。stable全般で不可能という永久的主張ではない。`#![no_core]` 等の特殊手法は今回対象外。

カスタムJSONは必須の唯一の方法ではない（組込みRV32EにC追加も考えられる）が、ここでは全coreも同じISAで作る構成を固定するため使用した。`features=+e,+c`, `llvm-abiname=ilp32e`, `abi=ilp32e`, stack alignment=4、atomic-cas=false、max-atomic-width=0。ハードウェアにA拡張はないため、原子的RMWは割り込み禁止等で実装する必要がある。

`riscv32imac-unknown-none-elf` は32レジスタ、ILP32、M（乗除算）/A（アトミック）を前提とするため、CH32V003へそのまま使用できない。単にアプリのフラグからM/Aを消しても、配布coreやABIが適合するとは限らない。`riscv32emc` もMを含むためそのままは不適切。

実際のELFでRVE/RVCフラグ、`rv32e2p0_c2p0_zca1p0`、4byte stack alignmentを確認。LLVMのC生成とRenvoでの実行も確認したが、LLVMの完全正当性の証明ではない。global_asm!による起動処理とmretハンドラ、asm!によるCSR操作がコンパイル・実行できた。Rust通常関数のプロローグより前に保存する必要があるため、コンテキスト切替入口は専用asm/naked境界を用いる。

`ch32-hal/examples/ch32v003` はRV32EC JSONとbuild-stdを使用し、現行JSONではxwchc/forced-atomicsも有効。本検証ではそれらを除外した。`ch32v00x-hal/Cargo.toml` はqingke/qingke-rtを利用し、J4M6を6 GPIOとして区別する。両HALの周辺実装は参考になるが、本検証の最小プログラムには依存させていない。

### QingKe V2とOS機能

WCH公式Startupは `mstatus=0x1880`, CSR 0x804=3（割り込みネストとhardware stack）、`mtvec=vector_base|3`、mepcを設定してmretする。標準RISC-V vectored mode 1の「base + 4*causeの命令へジャンプ」と、QingKe mode 3の「テーブルからハンドラアドレスを読む」は異なる。SysTickは標準CLINT/mtimeではなくWCH独自MMIOでIRQ12。公式Coreヘッダもmstatusの0x88による割り込み操作を使う。今回のハンドラはソフトウェア保存＋直接mtvecで、HPEの正確なレジスタ保存を確認していない。実機用にはqingke-rtの対応設定または公式仕様に沿う起動・ベクタ・保存方式を選び、両方式を混在させない。

|OS機能|エミュレータでの見通し|今回の確認|
|---|---|---|
|静的タスク生成・管理|固定2タスク、保存SP、静的スタック|追加検証で実装・実行|
|協調スケジューラ|yieldで固定2タスクを交互に実行|追加検証で32回の切り替え|
|プリエンプティブ|SysTick入口は利用可能。ネスト/HPE差に注意|SysTickのみ実行|
|コンテキスト保存|協調方式でx1..x15（sp含む）、raによる再開PC。タスク毎256バイト|追加検証でレジスタ・独立ローカル保持を確認|
|時刻管理/sleep|SysTickカウンタとソフトウェア時刻で機能検証可能|単発IRQのみ|
|トラップ|CSR、mret、PFICテーブル経路あり|直接IRQと上流mode3テスト|
|yield/sleep/write syscall|M-mode関数またはecall dispatcher。プロセス隔離はない|ecall dispatcher未実装|
|UARTシェル|TX確認済み。RX入力注入・端末連携の作成が必要|対話シェル未検証|
|静的メモリ管理|リンカ制限と境界負例を利用可能|静的8byteとスタック512byte予算|

MMUなし、Renvoの当該プロファイルはuser_mode=false。OSは同一アドレス空間の極小ランタイムとして設計する。2KBで多数の独立スタックを持つのは厳しい。まず2タスク程度の協調方式を選び、TCB/スタック/UARTバッファを静的に予算化する。

### 代替候補（動作比較のベンチマークは未実施）

- [QEMU公式RISC-Vボード一覧](https://www.qemu.org/docs/master/system/target-riscv.html): RISC-V CPUを実行できてもCH32V003機種とは限らない。今回CH32V003/PFIC/USARTモデルの実動証拠は得ていない。virt用カーネル検証をCH32実証と扱わない。
- [Renode](https://renode.readthedocs.io/en/latest/introduction/supported-boards.html): platformsを実際にcloneしch32/qingkeで検索したが該当なし（`renode-platform-search.log` は空）。CPUのRV32EC対応そのものは今回未検証。独自platform/PFICモデル追加なしに代替できるとは判断しない。
- [rv32emu](https://github.com/sysprog21/rv32emu): 主対象はRV32I[MAFC]。CH32V003メモリ・周辺回路の再現を確認していない。ビルド/実行未検証。
- [mini-rv32ima](https://github.com/cnlohr/mini-rv32ima): RV32IMA用。CH32上でこれを動かすプロジェクトはCH32をホストにした別CPUのエミュレーションであり、CH32のエミュレータではない。
- [Velxio](https://github.com/CHOUAiB/velxio): CH32V003/RV32ECのブラウザモデルを掲げる候補があるが、Rust ELF、ABI制約、PFIC忠実性は未検証。今回は代替として採用しない。

## F. 推奨開発構成

Apple Silicon macOSネイティブ + 固定nightly-2026-03-12/rust-src/rust-lld + RV32EC/ILP32Eカスタムターゲット + 固定Renvo + 今回のLMA変換スクリプトを選ぶ。Dockerを必須にせず、最初は静的メモリと協調2タスクから進める。実機段階でWCH-LinkEと対応する書込/デバッグツールを別途検証し、HAL/qingke-rtの起動・PFIC処理へ寄せる。

## 一次資料

- [Rust RV32E target](https://doc.rust-lang.org/rustc/platform-support/riscv32e-unknown-none-elf.html)
- [Cargo build-std](https://doc.rust-lang.org/cargo/reference/unstable.html#build-std)
- [LLVM RISC-V](https://llvm.org/docs/RISCVUsage.html)
- [RISC-V ILP32E ABI](https://docs.riscv.org/reference/abi/v1.0/riscv-cc-procedure-calling-convention.html)
- [WCH QingKe V2 manual](https://www.wch-ic.com/downloads/QingKeV2_Processor_Manual_PDF.html)（WebページはJS必須で本文取得不可。実装確認は下記公式配布ソースも使用）
- [WCH公式Coreヘッダ](https://github.com/openwch/ch32v003/blob/main/EVT/EXAM/SRC/Core/core_riscv.h)
- [WCH公式Startup](https://github.com/openwch/ch32v003/blob/main/EVT/EXAM/SRC/Startup/startup_ch32v00x.S)
- [ch32-hal](https://github.com/ch32-rs/ch32-hal/tree/f6b5c38a2399503e8d3984b4b51a2da3adc79b30/examples/ch32v003)
- [ch32v00x-hal](https://github.com/ch32-rs/ch32v00x-hal/tree/e06f58bb57a93e69df728b9146fd97d8114fbca0)
- [商品仕様](https://akizukidenshi.com/catalog/g/g118062/)
