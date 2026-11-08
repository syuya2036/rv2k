# rv2k

A tiny operating system written in Rust for RISC-V microcontrollers, designed to run within 2KB of RAM.

rv2kは、CH32V003の **RAM 2KB・Flash 16KB** で動くRust製の超小型OSを目指します。
TCP/IPスタックそのものをRustで自作し、タスクがSocket APIを通じて使うOSサービスとして提供します。
Ethernet NICはMAC/PHYとフレーム入出力を担い、TCP処理を外部プロセッサやオフロードモジュールへ委譲しません。

## 最初の統合完成目標: HTTP Echo APIサーバー

**MacからHTTPリクエストを送り、本文をそのまま返すAPIサーバーを、CH32V003J4M6＋NICで動かすこと**を最初の統合完成目標にします。
自作TCP/IPをOSのSocket APIとして提供し、Echo APIタスクがそのAPIを使って応答します。
ネットワーク処理とAPIアプリケーションの固定2タスクで、OS・IPC・ソケット・ドライバの連携を実証します。

初期仕様案:

- `POST /echo` の1エンドポイント。
- 本文は最大64バイト。JSON等への変換をせず、受け取ったバイト列を同じ長さで返す。
- 同時接続は1つ。1接続につき1リクエストを処理し、応答に `Connection: close` を付けて切断する。
- `Content-Length` が明示されたリクエストを受け付ける限定的なHTTP実装から始める。chunked転送は初期対象外とし、未対応の形式や上限超過を正常なEchoとして扱わない。
- ヘッダを逐次解析し、TCP受信がヘッダ・本文の途中で分割されても処理を継続する。リクエスト全体をRAMへ保存しない。
- 受信・送信の待機とタイムアウトをOSサービスで扱い、応答完了後も次の接続を受け付ける。

完成後の利用イメージ（IPアドレスは例。現時点では未実装）:

```bash
curl --data-binary 'hello rv2k' http://192.168.1.50/echo
# hello rv2k
```

受け入れ条件は、Macからの本文一致、空本文・64バイト本文、上限超過の拒否、分割受信、TCP再送、切断後の再接続を確認し、同じ実機用ビルドがRAM 2KB・Flash 16KBに収まることです。
64バイトという上限とHTTPパーサの予算は実装時に計測し、現在のTCP用メモリ予算に追加しても収まるかを検証します。

TCPシェルとOSユーティリティは、Echo API完成後、実機移植前の **Phase 10** に位置付けます。
[設計書](docs/design.md) のシェル向けアプリケーション案も、この後続拡張として扱います。
HTTPのメッセージ解析は [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112.html) を参照し、初期の限定実装をHTTP/1.1全仕様への準拠とはみなしません。

## 現在地

**Phase 4まで検証済み。Phase 5・6のソフトウェア基盤を実装し、実機・Mac接続ゲートを残しています。**
Apple Silicon macOS上で、RV32EC / ILP32EのRustプログラムをビルドし、Renvoで実命令を実行しました。

- 起動処理、RAM更新、UART出力、SysTick割り込みと復帰。
- 独立2タスク、各256バイトのスタック、協調方式で32回の切り替え。
- レジスタ・スタック上のローカル変数の保持と、復元欠陥を検出する負例テスト。

Phase 1ではREADY走査、タスクreturn・明示終了、終了後の単独継続をRenvoで確認しました。
全待機・wake・全終了の状態遷移はホストでも試験しています。
再現手順と制約は [タスク管理検証](ch32v003-rust-os-test/docs/scheduler.md) を参照してください。
Phase 2ではRV32E専用ecall、80byteトラップフレーム、ポインタ検査、公開task APIを検証しました。
実装済み番号と予約番号、計測・再現手順は [syscall検証](ch32v003-rust-os-test/docs/syscalls.md) を参照してください。
Phase 3ではsleep・イベント・64byte IPC・SysTick・全待機からの復帰を確認しました。
取りこぼし・wrap・割り込みマスク中の経過時刻を含む結果は [サービス検証](ch32v003-rust-os-test/docs/services.md) を参照してください。
Phase 4では公開mem snapshot、番兵監視、panic/illegal trap診断、実リンカによる容量超過拒否を検証しました。
容量・スタック・障害注入結果は [メモリ・障害検証](ch32v003-rust-os-test/docs/diagnostics.md) を参照してください。
Phase 5ではUART syscall、ピン所有権、software SPI、ENC28J60 streaming driverを追加しました。
ホストモデルとRenvoのMMIO検証はPASS、J4M6＋NIC実機は未保有のためPhase 5全体はPARTIALです。
試験範囲と未検証条件は [ドライバ検証](ch32v003-rust-os-test/docs/drivers.md) に記録しています。
Phase 6では仮想Ethernet、ARP・IPv4・ICMPのRV32ECフレーム注入試験を追加しました。
21フレームの正常応答・不正破棄とスタック番兵を確認し、実機NIC構成も容量内でリンクしました。
Macからの実pingはhost adapter経路が未検証のためPARTIALです。
試験範囲は [ネットワーク検証](ch32v003-rust-os-test/docs/network.md) を参照してください。
TCP、プリエンプション、CH32V003実機での動作はこれからです。
実測ログ・再現手順は [検証用プロジェクト](ch32v003-rust-os-test/README.md) と [協調マルチタスク検証](ch32v003-rust-os-test/docs/cooperative.md) を参照してください。

## 最終アーキテクチャ

|層|役割|
|---|---|
|Applications|HTTP Echo API Server（最初の目標）/ TCP Shell・Monitor・GPIO demo（後続拡張）|
|Public API / Syscalls|task / sleep / event / socket / write|
|Tiny Kernel|Scheduler / Task Manager / IPC・Sync / Timer / Memory・Stack / Syscall Dispatch|
|Kernel Services|自作TCP/IP / Network Driver / UART・GPIO / System Clock|
|Architecture / Hardware|RV32EC / QingKe PFIC / CH32V003 / ENC28J60|

内部はコンパクトなモノリシックカーネルとし、アプリケーションは公開APIを利用します。
カーネル構造体をアプリケーションから直接変更させません。
MMUによる分離がないため、この境界は論理的なAPI境界であり、セキュリティ上の保護境界ではありません。
サービスごとに独立したタスクやスタックを増やさず、ネットワーク処理とアプリケーションを固定2タスクで動かします。

## 統合ロードマップ — rv2k 2.0

ネットワークに入る前に、単体で動くカーネル、公開API、待機・通知、静的メモリ管理を成立させます。

|Phase|内容|完了条件|
|---|---|---|
|0 — 実証済み|Rust起動・協調マルチタスク|Renvoで独立2タスク・32回切り替え・レジスタ保持を確認|
|1 — 検証済み|タスク管理・スケジューラ|TCB、状態遷移、ラウンドロビン、タスク終了、実行可能タスクなしの処理|
|2 — 検証済み|システムコール基盤|関数APIを整理し、RV32E専用syscall ABI・dispatch・ecallを独立検証|
|3 — 検証済み|タイマー・IPC・同期|monotonic tick、sleep、イベント、固定待機情報、取りこぼしのない通知|
|4 — 検証済み|メモリ管理・障害処理|静的割当、スタック監視、panic・トラップ診断、RAM/Flash予算の自動検査|
|5 — PARTIAL|デバイスドライバ基盤|UART・GPIO・SPI・NetworkDevice、J4M6の配線とソフトウェアSPIを検証|
|6 — PARTIAL|仮想Ethernet・ARP・IPv4・ICMP|フレーム注入試験とMacからの実ping。ホスト接続経路も検証|
|7 — 次に実装|自作TCP/IP・Socket API|単一接続、listen / recv / send / close、再送・重複・切断・タイムアウト試験|
|8|プリエンプティブスケジューラ|SysTick強制切り替え、非再入カーネル、同期と待機の競合試験|
|9|HTTP Echo APIサーバー|POST /echo、最大64バイトの本文一致、分割受信、タイムアウト、再接続をMacから検証|
|10|TCPシェル・OSユーティリティ|TCP接続からps / mem / uptime / netstatを実行し、公開APIによるOS状態取得、分割入力、切断・再接続を検証|
|11|実機移植・耐久試験|CH32V003J4M6＋ENC28J60でEcho APIを実行し、実LAN、通信損失・再接続・長時間運転を検証|
|12|rv2k 2.0リリース|全機能を同じ構成で統合、RAM 2KB・Flash 16KB、仕様と再現手順を固定|

Phase 5まで待たず、**ピン割当と電気的成立性は設計段階の先行ゲート**とします。

Phase 10では固定2タスク・単一TCP接続を維持し、まずEcho APIとTCPシェルを選択式のアプリケーションとして実装します。Phase 11の実機移植・耐久試験では両構成を確認し、シェル追加後もRAM 2KB・Flash 16KBの制約を検証します。

## 2.0のサービス仕様

|サブシステム|目標仕様|
|---|---|
|タスク|固定2スロット、TCB、READY / RUNNING / BLOCKED / EXITED、終了処理|
|スケジューラ|ラウンドロビン、協調yield、後からSysTickによる強制切り替え|
|システムコール|固定番号、引数・戻り値・エラーコード、RV32E専用ABI|
|タイマー|monotonic tick、sleep、待機タイムアウト|
|IPC・同期|イベントフラグ、単一固定長メッセージ領域、割り込みマスク、クリティカルセクション|
|メモリ・障害|静的配置、スタック使用量・番兵検査、panic・トラップ原因の診断|
|ドライバ|UART / GPIO / SPI / Ethernet。パッケージごとのピン所有権を管理|
|ネットワーク|Rust製Ethernet / ARP / IPv4 / ICMP / TCP、単一TCP接続|
|ソケット|listen / recv / send / close、部分送受信と待機・通知|
|アプリケーション|まずHTTP Echo API。TCPシェルによるOS状態の確認・操作は後続拡張|

syscall ABI案は `a0=第1引数・戻り値`, `a1=第2引数`, `a2=第3引数`, `a5=番号`。
RV32Eに存在しないa6/a7は使いません。番号表、エラー、ブロッキング時の動作は [設計書](docs/design.md) に定義します。

## 小メモリTCPの設計方針

[RFC 9006 Appendix A.1](https://www.rfc-editor.org/rfc/rfc9006.html#name-uip) はuIPを小規模MCU向けTCP/IPの先例として紹介しています。
同RFCのsingle-MSS設計を参考に、rv2kは **単一接続・小さな受信窓・未ACKデータ1セグメント** から始めます。
uIPを組み込むのではなく、自作Rust実装の設計根拠として参照します。

rv2kではOSのSocket APIを安定させるため、送信データをACKまでカーネルの固定バッファに保持する案を採用します。
再送時にアプリケーションへデータの再生成を要求しません。
これは先例の存在を根拠にした開発方針であり、OS全体の2KB / 16KB収容は今後の統合検証で確認します。

## J4M6はピン配置が先行課題

J4M6にはハードウェアSPIのSCK=PC5・MOSI=PC6・MISO=PC7が外部に出ていません。
SPIのリマップでもこの3信号は別ピンへ移りません。
したがって、**J4M6本命構成は4線ソフトウェアSPI＋ENC28J60の周期ポーリング** とします。
[WCHデータシートのピン配置・機能表](https://akizukidenshi.com/goodsaffix/CH32V003.pdf) に基づく判断です。

配線案はPA2=SCK、PC1=MOSI、PC2=MISO、PC4=CS、PD6=UART TX、PD1=SWIO。
物理的に共通のPA1/PD6、PD1/PD4/PD5を別々のピンとして数えません。
NIC IRQと専用LEDは割り当てず、UARTはTXのみ。書き込み端子を確保し、MacからのHTTP Echo API通信を最初に検証します。
この案の電気的・速度的な実機確認は未実施です。

開発補助として同じRAM/Flash容量の多ピンCH32V003でハードウェアSPIを検証しても、J4M6での最終確認は省略しません。
ENC28J60内蔵のフレームバッファはNIC資源として別記し、MCUの一般RAMや外部CPUとして扱いません。

## 完成条件

以下を**同じリリース構成で全て**満たした時点を、rv2k 2.0の完成とします。

1. 独立スタックのタスクが協調・強制切り替えに対応する。
2. アプリケーションがシステムコール経由でOSサービスを利用する。
3. タイマー、IPC、メモリ監視、例外処理が機能する。
4. OS内部で自作Rust製TCP/IPスタックが動作する。
5. TCP Socket APIでネットワークアプリケーションを実装できる。
6. MacからHTTP Echo APIへ接続し、本文一致・分割受信・異常入力・再接続を確認できる。TCPシェルでは公開APIを通じてOS状態を確認できる。
7. 静的領域、全スタック、通信バッファを含めRAM 2KB・Flash 16KBに収まる。
8. RenvoとCH32V003J4M6実機＋NICの双方で動作する。

実装に向けた境界・予算・状態遷移・試験項目は [rv2k 2.0設計書](docs/design.md) を参照してください。
