# Phase 10: TCP シェル統合（仮想 NIC 検証済み）

Renvo 上の実 RV32EC 命令で TCP 接続から全 4 コマンドを実行し、独立 PCAP 検査が PASS しました。仮想 NIC 版は Flash 16,320 / 16,384 byte、全スタックを含む予約 RAM 1,964 / 2,048 byte です。ENC28J60 版は専有 SPI デバイスへの変更後も Flash が 1,616 byte 超過し、Mac native TCP と実機は未検証です。Phase 10 全体は未完了です。

## 実装

`--features shell` で T0=network、T1=shell の固定 2 タスクと単一 TCP 接続を使います。port 23 の plain TCP で、Telnet 制御は扱いません。banner は `rv2k shell`、prompt は `> ` です。

|コマンド|公開 API|出力|
|---|---|---|
|ps|task::with_info|2 タスクの PID、状態、終了コード|
|mem|mem::with_info|RAM / Flash、静的・スタック予約、使用高水位、番兵異常|
|uptime|time::uptime|u64 の tick と tick_ms=10|
|netstat|net::with_info|TCP 状態、flags、local / peer、フレーム数・エラー・再送・reset|
|help|—|コマンド一覧|
|quit|—|bye を送って接続終了へ進む|

実行部は公開 API のみを使います。application 領域の登録は別の firmware adapter が行います。`with_info` は syscall による task-local コピーを callback へ渡し、追加の構造体戻り値を避けます。callback 中に待機・preemption が起きてもコピーは変わりません。従来の `info()` も残します。snapshot probe は task / mem / net の各 callback で yield してコピーが変わらないことを確認します。

入力は最大 64 byte、出力も最大 64 byte の固定行です。出力オブジェクト内に 28 byte の十進変換用領域を再利用し、割り込まれる呼出し経路のスタックを抑えます。u64 全域の表示を保ち、ヒープも第 3 スタックも使いません。

入力は CR / LF / CRLF を扱います。超過・不正入力は次の改行まで破棄し、エラーを 1 回返して次の行へ復帰します。受信は 1 byte ずつ取り出し、出力中の未処理入力は kernel の TCP RX buffer に保持します。未完了行は実行せず、新しい接続で入力状態を reset します。

部分送信と Again を処理し、1 コマンドの送信期限は 500 tick、NET_TX_READY / NET_CLOSED を待ちます。対話入力は NET_RX / NET_CLOSED を期限なしで待ちます。quit 後は未処理入力を実行せず、最大 100 tick 排出して peer の FIN を待ちます。peer が先に閉じなければ active close となり、既存の 120 秒 TIME_WAIT が単一スロットを占有します。

## 再現と結果

```sh
cargo +stable test --workspace --all-features --locked
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cd ch32v003-rust-os-test
bash scripts/shell.sh
```

固定 nightly / 元の memory.x / 256 byte kernel stack / 既存のサイズ最適化フラグでビルドします。元 ELF の load span と RAM を検査してから、エミュレータ用 ELF を生成します。入力・整形・整数処理を含む workspace 80 tests、clippy、fmt が PASS です。

各 run は次の 5 接続を TCP MSS=24、受信窓=128 で検査します。

1. `ps / mem / uptime / netstat` を 1 byte ずつ入力。SYN-ACK 損失とゼロウィンドウ persist から復帰。
2. 65 byte 超過行、help、未知コマンド、不正制御文字、netstat、quit を 1 segment に連続入力。各エラー後に次の行を実行。
3. `up` の途中で EOF。banner 以外の出力なし。
4. `netsta` をアプリが読み取った後に RST。banner 以外の出力なし。
5. 再接続して uptime / ps。前接続の未完了行を引き継がない。

ホスト peer は CPU register や protocol state を書き換えません。checker は PCAP から両方向を独立に再構成し、checksum、再送 byte、入力一致、prompt / 全出力、公開 snapshot と ELF budget、カウンタ時点の handshake prefix、全 16 byte 番兵を照合します。起動時の GP / mem 専用 probe は shell には重ねず、専用 snapshot / preemption / syscall profile で検査します。

|測定|通常|RX 中 IRQ 注入|
|---|---:|---:|
|接続 / 入力 byte / コマンド|5 / 138 / 12|5 / 138 / 12|
|RX / TX frame|103 / 114|103 / 110|
|T0 / T1 / kernel 高水位 byte|204 / 228 / 112|204 / 224 / 112|
|強制切替 / 延期 IRQ|10 / 3|0 / 122|
|NIC 読出し中の 1 tick 注入|0|103|

ストレス試験では全 IRQ が T0 の保護された NIC 読出し中に届き、T1 が待機中なら強制切替は不要です。延期・共有借用の終了・全通信結果を検査します。強制切替自体は通常 run と、非 yield loop の専用 preemption probe（47 回）で確認しています。最大除外時間の計測は `preempt-timing` 専用で、shell の値は未計測です。

共通処理では word 除算の共有、送信 ACK 済み prefix の index 管理、送信結果処理の共有、snapshot byte view、検証範囲計算と stack watermark 走査の重複除去を行いました。TCP / snapshot 通常・IRQ 注入、SPI 途中 IRQ、レジスタ復元欠陥 3 例、syscall 70 trap と負例、preempt profile 障害 9 例、ARP / IPv4 / ICMP が変更後も PASS です。結果は [raw log / summary](logs/shell) と [共通部分の回帰](logs/shell/regression) に保存しています。

## 残る完了条件

ENC 版は次の元 linker 制約で失敗し、容量上限は変更していません。初回の超過は 2,360 byte（[当時の build log](logs/shell/enc-build.txt)）、専有 SPI デバイスへの変更後は 1,616 byte（[最新の build log](logs/spi-device/shell-enc-link-failure.txt)）です。[SPI 変更と検証](drivers.md) も参照してください。

```sh
RUSTFLAGS='-C opt-level=z -C target-feature=+relax -C llvm-args=-riscv-enable-global-merge=false -C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x -C link-arg=--icf=all -C link-arg=--relax -C link-arg=--relax-gp' cargo build --locked --release --features shell,drivers --target-dir target/shell-enc
```

ENC 構成の収容、Mac native TCP、統合 PR のレビュー / マージが残ります。実機は未保有です。仮想 packet peer の成功を Mac や実機の PASS として扱いません。
