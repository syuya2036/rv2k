# Phase 10: 公開 OS snapshot API

TCP シェルがカーネル構造体を参照せず状態を取得するための API です。既存の task / mem / uptime に、予約済み番号 14 の network snapshot を追加しました。シェルのコマンド実行と socket 出力は後続作業です。

|API|syscall|出力|alignment|
|---|---:|---:|---:|
|`task::info(id)`|12|TaskInfo 6 byte|2|
|`mem::info()`|13|MemInfo 24 byte|2|
|`time::uptime()`|3|tick u64 8 byte|8|
|`net::info()`|14|NetInfo 32 byte|2|

アプリは取得したコピーを扱い、内部 TCB / socket / NIC へのポインタを取得しません。各呼出しの時点の snapshot であり、後の状態変化は反映されません。異なる API を続けて呼んだ結果は同じ時刻の一括 snapshot ではありません。M-mode 単一アドレス空間の論理的な API 境界であり、メモリ保護の提供ではありません。

## NetInfo v1

`a5=14, a0=出力先, a1=32, a2=0`、成功時 `a0=0` です。`rv2k_api::net::info()` がこの ABI を使います。ネットワーク非搭載構成は NoSys、サイズ違い / reserved 引数の非ゼロは Invalid、不正な alignment / 範囲 / 加算 overflow は Fault です。書込み先は呼出しタスクの live stack または登録済み application 領域に限ります。

両タスクが読める状態照会です。socket の所有権を変更せず、イベントや受信データを消費せず、DRIVER_POLL を発行せず、待機しません。ネットワークタスク終了後も最後の状態を読み、flags に NETWORK_STOPPED を設定します。snapshot は共通 trap dispatch の IRQ 除外中に同期的にコピーし、ユーザーポインタを保持しません。

整数は little-endian、IP は network-order の 4 octet、予約 padding なしの 32 byte です。

|offset|フィールド|byte|
|---:|---|---:|
|0|version=1|2|
|2|TCP state|1|
|3|flags|1|
|4 / 8|local / peer IPv4|各4|
|12 / 14|local / peer port|各2|
|16 / 18|受信 / 送信フレーム数|各2|
|20 / 22|drop / bad checksum|各2|
|24 / 26|fragment / I/O error|各2|
|28 / 30|TCP retransmission / reset|各2|

TCP state は Closed=0、Listen=1、SynReceived=2、Established=3、FinWait1=4、FinWait2=5、Closing=6、CloseWait=7、LastAck=8、TimeWait=9 です。同じ公開型を TCP 実装でも使います。

flags は LINK_UP=1、PEER_PRESENT=2、NETWORK_STOPPED=4 です。link / TCP state は最後に network が処理した状態です。たとえば network task の終了後、キャッシュされた LINK_UP と NETWORK_STOPPED が同時に立つことがあります。peer 不在時の IP / port はゼロです。全カウンタは 65,535 で飽和します。

## 再現と結果

```sh
cargo +stable test --workspace --all-features --locked
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cd ch32v003-rust-os-test
bash scripts/snapshot.sh
```

ホスト試験は固定 wire image、実 TCP フレーム処理に伴う状態 / 再送 / reset、コピーの独立性、両 dispatcher、両 caller、不正引数 / ポインタ、network 終了後、未搭載時の NoSys を確認します。

`netinfo-probe,emulator-stop` は公開 API をゲスト上で呼び、起動前、接続 1、接続 2 のコピーを観測領域へ保存します。サイズ / reserved 引数 / 不正出力先の 7 ケースも raw ecall で確認します。観測領域は syscall の出力先として使わず、コピー先は通常の task stack です。この probe は試験専用であり production feature には含めません。

独立した Python checker は 32 byte のレイアウトを解析し、保存されたカウンタ時点の PCAP prefix に対応する handshake があることを確認します。先行する TCP checker は全 PCAP の checksum、本文、損失回復、全スタック番兵を検査します。

通常 / RX 中の IRQ 注入とも 2 接続・193 byte echo が PASS、boot / 接続中の state は 0 / 3 / 3、peer port は 0 / 50000 / 50001、再送数は 0 / 1 / 3 でした。通常の RX snapshot は 0 / 2 / 14、TX は 0 / 2 / 17 です。ストレス時の最後の TX は 16 です。タイミングで変わる値を固定の期待カウンタとして流用せず、PCAP prefix と照合しています。

callback の yield 後コピー不変（task / mem / net）も追加検査します。

Probe ELF は Flash 14,860 byte、予約 RAM 1,932 byte（観測領域を含む）です。T0 / T1 / kernel 使用量は 176 / 216 / 120 byte、全 16 byte 番兵が正常です。シェル統合も別 profile で全番兵と高水位を検査します。

Probe を含まない `tcp,preempt,drivers` の元 ELF は Flash 14,892 / 16,384 byte、予約 RAM 1,844 / 2,048 byte でリンクしました。UART / SPI の実測高水位はこのリンク検査で証明していません。CH32V003J4M6＋ENC28J60 は未保有で、実機と Mac native TCP は UNVERIFIED です。

[raw log / summary / 容量検査](logs/snapshot) と [共通部分の回帰結果](logs/snapshot/regression.json) を保存しています。既存の CPU register / PC 復元、SPI 途中 IRQ、guard、復元欠陥の負例は変更後も PASS です。

最適化構成は `-Z inline-mir=yes` を含みます。期限付き待機の追加後の共通回帰は [deadlines](logs/deadlines) を参照してください。
