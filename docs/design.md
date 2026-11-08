# rv2k 2.0 設計書

状態: Phase 4まで検証済み。Phase 5はsoftware driver/model/RV32ECがPASS、J4M6実機未検証のためPARTIAL。Phase 6はvirtual NICとARP/IPv4/ICMPのhost/RV32ECフレーム注入がPASS、Mac実ping未検証のためPARTIAL。Phase 7のTCP/Socket基礎はhost試験中（wire/guest未接続）。Phase 8以降は設計案。Phase 0/1の実測を土台に、後続の契約と検証条件を定める。設計上の予算やAPIを実装済みと解釈しない。リリース名は **rv2k 2.0** に統一する。

## 1. 目的と決定事項

CH32V003J4M6のMCU内RAM 2048バイト、Flash 16384バイトで、カーネル・システムコール・タスク・IPC・ドライバ・Rust製TCP/IPを統合する。外部汎用RAM、SDカード、外部TCP処理プロセッサは使わない。

- 内部は単一アドレス空間のモノリシックカーネル。アプリケーションとの境界は公開APIで定義する。
- 固定2タスク: T0=network service、T1=application（TCP shell / echo / monitorを同時に別タスク化せず、同じスロットで選択）。idleは第3タスクにしない。
- タスクスタック各256バイト。カーネル／非ネストIRQ用スタック256バイトを、起動後も共用する目標構成。
- まず協調方式、次にecallと待機機構、最後にプリエンプション。クリティカル区間やカーネル実行中にはタスクを強制切替しない。
- NICはENC28J60。MAC/PHYとフレームバッファだけを利用し、Ethernetより上のプロトコル処理とチェックサムをRustで実装する。
- 初期ネットワークプロファイルは同一LAN、静的IPv4/MAC、1 TCP接続、受動open、MSS目標128、未ACK送信データ最大128バイト。性能より相互運用とサイズを優先する。
- 本書の数値目標は各Phaseの実測で検証する。サイズ超過時にNICへTCPを移して達成扱いにしない。

[RFC 9006](https://www.rfc-editor.org/rfc/rfc9006.html#name-uip) のuIPは小メモリTCPの先例である。rv2kはsingle-MSS方式を参考にするが、OSサービスとして送信データの再送責務をカーネルに置く。アプリケーションに再送時のデータ再生成を要求するuIPの方式をそのまま採用するわけではない。

## 2. Phase 0からの差分

Phase 0の [協調検証](../ch32v003-rust-os-test/docs/cooperative.md) は独立2タスク・各256バイト・32回切替を実証している。固定xor切替、IRQ禁止、テスト用ebreak停止であり、BLOCKED/EXITEDや一般的なsyscallはまだない。

Phase 0実測値: 静的RAM592バイト、起動スタック予約512バイト、Flash text1553バイト。以下の1792バイト予算は、テスト用状態の縮小、起動スタックの変更、ドライバ・TCPの追加を伴う将来設計。現在の領域に単純加算して収まるという意味ではない。

実機ELFとRenvo用LMA補完ELFの分離を継続する。Phase 6でRenvoの仮想NICとフレーム注入を追加した。Macへのネットワーク接続は未検証。ENC28J60のSPIモデルはPhase 5のホスト試験で使い、Renvoの物理ENCとしては接続していない。

## 3. ハードウェア成立性を先に検証する

### 3.1 J4M6のパッケージ制約

[WCH CH32V003データシートV1.3](https://akizukidenshi.com/goodsaffix/CH32V003.pdf) の2.1〜2.3節に基づく配線案。SOP8にはSPI1のPC5/PC6/PC7が出ていない。SPI1の別機能マッピングもこの3信号のピンを変えないため、J4M6ではハードウェアSPIを前提にしない。

|物理ピン|利用するパッド／機能|接続先・条件|
|---|---|---|
|1|PD6 / USART1 TX（remap 2）|UART TX専用。共通ピンPA1は出力せず、外部発振器を使わない|
|2|VSS|共通GND|
|3|PA2 / GPIO出力|ENC SCK（ソフトウェアSPI）|
|4|VDD|MCUとNICの信号電圧を合わせた3.3V系を候補とし、電源回路は別途検証|
|5|PC1 / GPIO出力|ENC SI（MOSI）|
|6|PC2 / GPIO入力|ENC SO（MISO）|
|7|PC4 / GPIO出力|ENC CS。起動中に非選択となる外部プルアップ|
|8|PD1 / SWIO|書込み／デバッグ専用。共通ピンPD4/PD5から出力させない|

別名のパッドを独立した物理ピンとして割り当てない。USARTのRXは無効にし、remap 2のRX=PD5とSWIOの同時使用を避ける。既存検証のPD5 TX設定は、このボード設定ではPD6 TXへ変更する。

NIC INTは接続せず周期ポーリング。NIC RESETは常時非アクティブとなる回路とSPI soft resetを使用する案。CLKOUTは未接続、NIC自身のクロックはNIC側で用意する。MCUにはHSIを使う。RESET回路、電源容量、SPIモード／上限速度、各共通パッドの初期化順序は、基板確定前にNICデータシートと実測で確認する。

この構成は6本の信号ピンを使い切る。専用LEDやUART RXを同時追加しない。GPIO APIは存在しても予約済みピンへの要求にはBusyを返す。LED demoは別ボード／別ビルド構成で検証する。

### 3.2 先行ゲート

1. ピン表と物理ボードの導通、未使用別名パッドの非駆動を確認。
2. SWIOを保持したまま繰り返し書込みできることを確認。
3. NICレジスタの読書き・soft reset・リンク状態取得をソフトウェアSPIで検証。
4. ポーリング周期、1フレームの転送時間、割り込み禁止時間、RXオーバーフローを測る。
5. 多ピンのCH32V003F4P6でハードウェアSPIを対照検証しても、J4M6のゲートを代替しない。

ソフトウェアSPIの時間はCPUコストになるため、RAMより先にピンと転送時間がボトルネックになる可能性がある。これは未測定であり、速度の達成値はまだ置かない。

ENC28J60にはNIC内フレームRAMがある（[Microchip製品仕様](https://www.microchip.com/en-us/product/enc28j60)）。これを外部汎用RAMとしてOSのスタックやTCBに使わず、MCUの2KBと別会計にする。TCPの再送データはMCU内128バイトに保持する。

## 4. ソース構成と依存方向

既存の検証ディレクトリは実測履歴として維持する。Phase 1で本体とホスト試験を次の構成へ段階的に分離する案:

```text
crates/
  rv2k-api/        # syscall番号、エラー、公開snapshot型、ユーザー側wrapper
  rv2k-kernel/     # scheduler / task / event / timer / ipc / diagnostics
  rv2k-net/        # no_std、MMIOに依存しないパーサとプロトコル状態機械
  rv2k-drivers/    # UART / GPIO / SPI / ENC / virtual NIC
  rv2k-arch/      # RV32E context / trap / QingKe / critical section
firmware/
  ch32v003/       # board pin ownership、静的配置、リンク、タスク入口
host/
  net-harness/    # PCAP、フレーム注入、損失・遅延・重複、Mac接続
```

api/kernelはPhase 1、archはPhase 2、driversはPhase 5、netとhost/net-harnessはPhase 6で導入済み。firmwareは現在ch32v003-rust-os-testに配置する。アプリはapiだけを参照し、kernel内部TCBやsocket構造体を公開しない。netはdriverの具体型を知らず、静的ディスパッチのNetworkDeviceを使う。firmwareにtrait object、ヒープ、動的なタスク／socket生成を導入しない。unsafeはarch、MMIO、静的領域の限定した所有権境界へ集約する。

## 5. タスク管理とスケジューラ

### TCBと状態

固定2スロットにTaskId=0/1を割り当て、動的再利用は初版では行わない。各TCBの目標上限は32バイト。

候補レイアウト（RV32、repr(C)、実装時size_ofで検証）:

|フィールド|バイト|
|---|---:|
|saved_sp / entry / deadline_low / wait_mask / pending_events|4×5|
|wait_object|2|
|id / state / wait_reason / flags|1×4|
|exit_code|2|
|completion_result|4|
|合計|32|

状態遷移:

- 初期化 → READY。選択 → RUNNING。
- RUNNING → READY: yield、または許可された場所での量子満了。
- RUNNING → BLOCKED: sleep、未成立イベント待機、メッセージ待機。
- BLOCKED → READY: 条件成立、タイムアウト、相手終了によるキャンセル。
- RUNNING → EXITED: task_exitまたは入口関数からのreturn。
- EXITEDは選択しない。再起動はシステム再初期化で行う。

スケジューラは現在の次スロットからREADYを最大2回走査する。現在タスク自身だけが実行可能なら同じタスクへ戻る。`CURRENT_TASK ^= 1` の無条件切替を廃止する。状態変更と選択は単一の短いクリティカル区間で行い、RUNNINGは最大1つ。

全タスクがBLOCKEDならカーネルスタックでidleする。tickやNICポーリング期限で待機を再評価する。初期版はIRQ許可付きの有限ポーリングを用い、WFIはQingKeでの取りこぼしを独立検証してから使う。全タスクがEXITEDなら診断後に期待停止する。ネットワークタスク終了時はsocketを失敗状態へ移し、待機アプリへ通知する。

タスク入口はtrampoline経由とし、entryがreturnしたら `task_exit(0)`。終了コード記録後にREADYへ戻さず、現在タスクのスタックから実行中に自身の領域を解放しない。固定割当なのでfree処理は不要。

## 6. コンテキスト・トラップ・クリティカル区間

Phase 1は既存の64バイトRV32E協調フレームを利用する。ra、gp、tp、t0〜t2、s0〜s1、a0〜a5、元spを保存。x0は不変、x16以降は使わない。初回entryと再開時レジスタを明示初期化する。

Phase 2でecall共通の80バイトフレームへ移行済み。IRQ統合はPhase 8:

|オフセット|内容|
|---|---|
|0〜56|既存64バイトフレームと同じ15レジスタ（sp含む）|
|60|mepc|
|64|mstatus|
|68|mcause（診断）|
|72|mtvalまたは該当QingKeで利用可能な診断値|
|76|予約・アラインメント|

[Phase 2検証](../ch32v003-rust-os-test/docs/syscalls.md) は下記の交換・検査・復帰をRenvoで実証した。実機は未検証。
トラップ入口はRust関数のプロローグより前の専用asmとする。`mscratch` にkernel stack topを準備し、最初にspを交換してカーネルスタック上の一時80バイトへレジスタと元spを退避する案。scratchレジスタを使う前に保存する。元spと80バイト分の範囲・アラインメントを検査してから、タスクスタックへフレームをコピーしTCB.saved_spを更新する。壊れたspに対して先にstoreしない。カーネル側の一時フレームはdispatch前に解消し、同じkernel stackを再利用する。この交換経路とQingKeのmscratch動作はPhase 2で実証する。

通常のecallでは保存mepcを4進める。IRQでは進めない。ecallとIRQの復帰は保存mepc/mstatusとレジスタを復元してmret。初回起動もtrampolineをmepcに置く。カーネル内の同期例外は再開を試みずfatal診断とする。MIE禁止だけで同期例外が抑止されるとは考えない。

固定2タスクの全てをM-modeで動かす。ecallは権限分離を作るものではない。QingKeのmtvec mode 3やHPEは標準RISC-Vのvectored modeと同一視しない。初期の統合方針はソフトウェア保存・IRQ非ネストとし、CSR 0x804/HPEとPFICを実機で合わせる。ハードウェア保存とソフトウェア保存を重ねて使わない。

カーネル実行中のMIEは無効とし、カーネル自体は非再入。初期化後のブロッキングsyscallはカーネルのRustコールスタックを残して切り替えず、待機情報をTCBへ記録してカーネルスタックを巻き戻してから選択する。

256バイトのカーネルスタックと80バイトのタスク上フレームは目標。現在の64バイト検証のwatermarkをそのまま将来の余裕とみなさない。移行時にcall chain全体を再測定し、canary破壊または予算超過ならPhaseを完了にしない。

クリティカル区間は入口のMIE状態を保存し、出口で元の状態を復元する。内側の解除で外側のIRQ禁止を解除しない。区間内のyield/sleep/event_waitは禁止。MMIOアクセスのvolatileとコンパイラ順序制約を定め、SPI全転送・checksum計算・UART待機をIRQ禁止区間へ入れない。

Phase 8のSysTickはタスク実行中にrescheduleを要求する。カーネル実行中や、共有状態更新の途中はその要求を復帰時／サービス境界まで保留する。ネットワークタスクがTCP状態やNIC借用を持つ間は、短い `preempt_disable` 区間を設けて切替だけを延期し、長いフレーム転送全体のIRQをマスクしない。区間の最大時間を計測する。

ネットワークサービスのパーサやSPIトランザクションは完了した有限の単位で切替可能にする。途中状態を別タスクのsocket syscallへ見せず、資源を使う `&mut` やデバイス所有権を跨いで別の利用者へ切り替えない。network taskだけが所有する処理と、syscallで共有する短いmailbox更新を区別する。

## 7. 公開APIとsyscall ABI（案）

最初はRust関数APIを実装し、同じ意味論をecall wrapperとdispatchへ移す。
[ILP32E ABI](https://docs.riscv.org/reference/abi/v1.0/riscv-cc-procedure-calling-convention.html) に存在するa0〜a5だけを使い、Linux ABIを流用しない。

- a0/a1/a2: 最大3引数。a5: syscall番号。
- a0: 成功値または負のエラーをisizeで返す。その他のcaller-savedレジスタはwrapper契約に従う。
- 64bit値・snapshotは検査した出力ポインタへ書く。番号と構造体versionをPhase 11で固定する。
- 以下の番号は草案で、0〜10は提示された規約を保持する。

|番号|呼出し|引数と戻り値|
|---:|---|---|
|0|Yield|引数なし、0|
|1|Sleep|a0=相対tick、復帰時0。0はyield|
|2|GetPid|TaskId|
|3|Uptime|a0=8byte出力先、a1=サイズ、0。起動後u64 tick|
|4|EventWait|a0=mask、a1=相対timeout、成立bitsetまたはTimeout|
|5|EventSignal|a0=宛先TaskId、a1=mask、0|
|6|Write|a0=fd（UART=1）、a1=ptr、a2=len、受理byte数|
|7|SocketListen|a0=port、唯一のsocket handle。接続待ちはイベントで行う|
|8|SocketRecv|a0=handle、a1=ptr、a2=capacity、受信byte数／EOF=0／Again|
|9|SocketSend|a0=handle、a1=ptr、a2=len、コピーしたbyte数／Again|
|10|SocketClose|a0=handle、close開始=0。完了はCLOSEDイベント|
|11|TaskExit|a0=終了コード、戻らない|
|12|TaskInfo|a0=TaskId、a1=snapshot出力、a2=サイズ|
|13|MemInfo|a0=snapshot出力、a1=サイズ|
|14|NetInfo|a0=snapshot出力、a1=サイズ|
|15|MessageSend|a0=宛先TaskId、a1=ptr、a2=len（最大64）|
|16|MessageRecv|a0=ptr、a1=capacity、実byte数／Again|

エラー案: Invalid=1、NoSys=2、BadFd=3、Again=4、Timeout=5、Busy=6、Closed=7、Fault=8、MessageSize=9、Io=10。a0ではそれぞれ負値。未知番号はNoSys、未知フラグ・不正長はInvalid。snapshotは固定長・version付きで、可変長formattingをsyscall内に持ち込まない。

公開APIは `task::yield_now/exit/info`, `time::sleep/uptime`, `event::wait/signal`, `ipc::send/recv`, `socket::listen/recv/send/close`, `console::write`, `mem::info`。Rustの予約語yieldを関数名にそのまま使わない。

### ポインタとブロッキング

入力はlen加算のoverflow、アクセス範囲、型のalignmentを検査する。書込み先は呼出しタスクのスタックまたは専用application領域に限定し、kernel/他タスクのメモリは拒否する。読み取りはそれらに加えて許可したFlash定数領域を認める。これは誤用検出であり、MMUなしで悪意ある直接アクセスを阻止する仕組みではない。

SocketRecv/SendとUART Writeは初期APIでは非ブロッキング。カーネルはユーザーのslice/referenceやポインタをsyscall復帰後まで保持しない。Sendは最大128バイトを固定TX領域へコピーして返す。Recvは保持済みRXデータからcapacityまでコピーして返す。未成立ならAgain、ユーザー側wrapperがイベント待機して再試行する。部分送受信を正常な結果とし、メッセージ境界をTCPへ仮定しない。

## 8. 時間・イベント・IPC

Phase 3の実装・実測と実機の残ゲートは [サービス検証](../ch32v003-rust-os-test/docs/services.md) を参照。

タイマーベース目標は10ms tick。u64 uptimeと、下位u32のdeadlineを使用する。相対timeoutは2^31 tick未満とし、wrapを考慮した差分比較を行う。0は非待機、0xffffffffは無期限待機を示す予約値。SysTickの実時間精度、マスク中の経過時刻補正、カウンタwrapを別々に検証する。

イベントはタスクごとのpending bitset。NET_RX、NET_TX_READY、NET_CONNECTED、NET_CLOSED、MSG_AVAILABLE、DRIVER_POLLを予約する。EventWaitは指定maskのany条件とし、返したsticky bitを消費する。socketのデータ有無などはレベル条件でも再評価する。

受信待ちの順序:

1. Shell TaskのrecvがAgainを返す。
2. EventWaitがIRQ禁止区間でpendingとsocket readinessを再確認する。
3. 未成立ならwait_mask/deadlineとBLOCKEDを一緒に設定してスケジュールする。
4. Network TaskがRX領域へコピーし、ready状態とイベントを設定する。
5. 待機条件に一致したShell Taskを一度だけREADYにする。
6. 再開したアプリがrecvを再試行する。

通知とBLOCKED設定の間でwakeを失わない。条件成立とtimeoutが同tickなら、既に成立した条件を先に処理する。イベントは回数を数えず、メッセージの実数や残量は各資源状態で確認する。

固定2タスクなので待機キューはTCBのwait_reason/object/maskを走査して実装し、リンク構造や動的ノードは置かない。IPCメッセージは全体で64バイト1スロット、宛先・長さ付き。満杯はAgain、サイズ超過はMessageSize。ネットワークのTCP payload領域とは共有しない。

T0のネットワークタスクはtickからDRIVER_POLL通知を受け、NIC受信・TX完了・ARP/TCP期限を順に処理する。IRQなしのJ4M6でも有限周期で受信を監視し、イベント待ちで永久停止しない。協調方式ではタスクがyieldするまで応答できないため、1回の処理フレーム数／SPIチャンクを制限する。

## 9. ドライバ境界とバッファ所有権

### UART / GPIO / SPI

UARTはTX非ブロッキング、32バイトリングを基準とし、満杯なら部分writeまたはAgain。RXを含む64バイト枠はボード依存で再配分可能。UART故障や未接続でOS全体を永久に待たせない。

ピンはboard設定で所有者を1つに固定する。J4M6の各物理ピンと別名パッドをまとめて所有権検査し、アプリにNICのCSやSWIOをGPIOとして開放しない。

SPIトランザクションはCS開始〜終了の所有権をT0のドライバだけが持つ。J4M6はsoftware backend、多ピン検証はhardware backend。各バイト中の割り込み可否と最大CS保持時間を測り、CSを共有したまま別タスクがSPIを操作する構成にしない。

### NetworkDevice契約

Phase 5でtraitとENC backendを実装済み。[ドライバ検証](../ch32v003-rust-os-test/docs/drivers.md) に試験範囲・実機の残ゲートを記録する。

フルEthernetフレームをMCUのスタックへ載せない。デバイス側にフレームを保持したまま、固定scratchでチャンク転送する。

|操作|契約|
|---|---|
|poll|リンク、RX有無、TX完了を更新。実行量は有限|
|rx_begin|受信フレームhandleと長さを返す。FCSは除外|
|rx_read_at|範囲検査後、固定scratchへ部分コピー。checksum用の再読取りを許可|
|rx_release|パケットごとに必ず一度実行し、NIC ringを進める|
|tx_begin|出力長を宣言。TX使用中ならAgain|
|tx_write|順序付きの部分書込み。アプリのポインタを保持しない|
|tx_commit / abort|検証済みフレームだけ送信。エラー時は予約解除|

借用中にyieldしない。RX処理はchecksumと境界検査を完了してからTCP stateや受信済みデータを更新する。エラーも含む全経路でrx_releaseする。NIC TXがbusyなら、ACK/ARP応答などを小さいpending actionとして保持し、共有scratchへフレームを保持したまま待たない。

プロトコル状態はT0が変更し、アプリのSocketSend/Recvは短いカーネル処理でmailbox/リングと操作要求だけを更新する。IRQはNICやTCPへ再入せず、時刻・通知・reschedule要求だけを扱う。

## 10. ネットワークプロファイルとTCP

### Ethernet / ARP / IPv4 / ICMP

初期プロファイルは単一LAN・静的MAC/IP、ARP cache 1エントリ。宛先MAC解決前にユーザーデータをACK済みとして扱わない。ARP更新・失効・再試行は固定期限で行う。フレーム長、EtherType、宛先、IPv4 version/IHL/total length/TTL/header checksum、上位checksumを検査する。

IPv4 optionはIHLを見てpayload位置を計算し、不正長を拒否する。初版はIPフラグメントを再構成せずdropしてカウンタへ記録する。この制限は一般的なIPv4ホストの完全準拠とは異なるため、リリース時の適用範囲に明記する（[RFC 1122](https://www.rfc-editor.org/rfc/rfc1122.html) の再構成要求と区別）。MSSを小さく広告するだけで全てのIPフラグメント問題が消えると仮定しない。

ICMP Echoは通常のMac ping相当の非分割フレームをNIC内からチャンク処理し、ペイロードを保ったEcho Replyを生成する。TCP RX/TX128バイトという制限を、Ethernetフレーム全体やICMPの上限と混同しない。IP/ICMP checksumは奇数長・チャンク境界を保持した逐次加算で計算する。

### 単一接続・single-MSS TCP

TCPの正当性は [RFC 9293](https://www.rfc-editor.org/info/rfc9293/) を基準に、縮小方式は [RFC 9006 §3.2](https://www.rfc-editor.org/rfc/rfc9006.html#name-tcp-guidance-for-single-mss-) を参照する。機能を絞る際は省略と非準拠を区別し、制限付きプロファイルの適合表をPhase 7で作成する。

- 受動openのみ。listen後にSYNを受理して同じslotを接続状態へ移す。別acceptキューは持たない。
- `CLOSED → LISTEN → SYN_RCVD → ESTABLISHED`。closeはFIN_WAIT_1 / FIN_WAIT_2 / CLOSING / CLOSE_WAIT / LAST_ACK / TIME_WAITを区別し、同時closeと再送FINを処理する。
- 接続はremote IP/portとlocal portで識別。別peerや別socket世代のパケットを現在のデータとして受け入れない。
- seq/ackはu32のwrapを考慮して比較し、SYN/FINによる1の消費を含める。snd_una / snd_nxt / rcv_nxtを別々に保持する。
- 受信payloadリングは128バイト。MSS案128、未ACK送信payloadも最大128バイト。送信量はpeer MSS・peer window・ローカル容量の最小に従う。
- RXの現在空き容量を広告するだけでなく、既に広告した受信窓の右端を不用意に後退させない管理を行う。満杯時は受信窓0、消費後はwindow updateを送る。
- in-orderのみをバッファし、重複先頭部分は切り落とす。順不同部分は保持せず、現時点の累積ACKを返す。受理窓外をメモリにコピーしない。
- 送信TX128バイトはACKまで不変。部分ACKでは未ACK suffixを保持する。累積ACKを重複適用せず、未送信範囲をACKする値はrejectする。
- SYN/FINにも再送期限を設定する。payload再送では元のseqとbyte列を再現し、shellの現在の状態から再生成しない。
- 受信ACKは基本即時送信。peerのDelayed ACKを許容し、スループット低下を故障と誤判定しない。
- MSS/NOP/EOLを処理し、未知optionはlengthを検証してskipする。未交渉のwindow scale等を勝手に適用しない。
- RST、zero-window、受信EOF、close中のデータ、再送上限、TIME_WAITを状態機械の試験に含める。

RTOは [RFC 6298](https://www.rfc-editor.org/rfc/rfc6298.html) を基準に、初期1秒、SRTT/RTTVAR、Karn方式、指数backoffを固定小数点で実装する案。遅いpeerのACKを待たずに過剰再送しない。再送上限案は8回とし、到達時にTimedOut状態をアプリへ通知する。peer window=0は別persist timerでprobeし、通常のデータ再送回数だけで正常な窓閉鎖を切断しない。失敗した接続から閉鎖状態へ移った後は世代を更新する。

送信flightを1セグメントに固定し、容量拡大のための一般的な大きなsliding windowを実装しない。single-MSSを根拠に輻輳・flow controlやRTOを省略せず、より積極的な送信になっていないことを確認する。高速再送等の扱いは [RFC 5681](https://www.rfc-editor.org/rfc/rfc5681.html) とsingle-MSSの適用範囲を適合表で照合する。

初期seqは単なる固定0にしない。生成と再起動時の扱いはPhase 7で試験し、暗号学的安全性を持たない場合は明記する。TCPシェルの初版は閉じた開発LANを対象とし、TLSや認証を実装したとみなさない。

### Socketサービスとアプリ

socket handleはslotと世代を含む整数。1つのlistener/connectionをT1が所有する。TIME_WAITを含め状態を解放するまで別接続へslotを再利用しない。接続終了後もlistener設定は保持し、同じ所有者が次の接続を待つ。

`send` 成功はkernel TX領域へのコピー完了であり、peerへの到達保証ではない。`recv` はストリームから最大capacityを返し、EOFは受信済みデータを全て読み終えた後の0。状態異常と一時的なAgainを区別する。アプリはNET_RX/TX_READY/CONNECTED/CLOSEDを待ち、APIを再試行する。

シェルは行バッファ64バイト、超過行は次の改行まで破棄してエラー応答。ps/mem/uptime/netstatは公開snapshot APIを使う。長い出力は小さなチャンクでsocketへ送る。shellとechoの切替はアプリ内部モードとし、第3スタックを要求しない。

## 11. 静的メモリとFlash予算

現在の実測と予算の自動検査は [Phase 4検証](../ch32v003-rust-os-test/docs/diagnostics.md) を参照。

**目標予算。現時点のsize_ofやリンク結果ではない。** データ、padding、canaryを各枠に含め、実装後にlinker mapで実数を出す。

|RAM用途|上限案（byte）|
|---|---:|
|タスクスタック2本（context frameとcanaryを含む）|512|
|カーネル／IRQ／起動共用スタック|256|
|TCB 2個|64|
|時刻・scheduler・event・IPC管理＋64byte message|96|
|TCP connection state|128|
|ARP・MAC/IP設定|64|
|TCP受信payload|128|
|TCP未ACK送信payload|128|
|header scratch 96＋stream scratch 64|160|
|UARTリング・状態|64|
|shell行領域|64|
|SPI・NICドライバ状態|64|
|診断・カウンタ|64|
|小計|1792|
|未割当の予備|256|
|MCU SRAM上限|2048|

TCBのpending eventsとkernel共通状態を二重計上しない。NIC内RAM、ホストのPCAP、実行ログはMCU容量へ算入しない一方、スタック上の配列、コンパイラのspill、RTT計算のローカル等は必ず算入する。大きな構造体の値渡しや再帰を避け、全バッファに所有者と寿命を定める。

Flashの作業予算案: arch/kernel/syscall 5KB、driver 3KB、protocol 6KB、shell/diagnostics 1KB、予備1KB。16進KBではなくここでは1KB=1024byteとする。core/compiler_builtins、rodata、data初期値、alignmentも含め16384以下をリンクで検査する。既存の1553byte測定からTCP統合後の収容を断定しない。

## 12. 検証構成とPhaseの受け入れ条件

全Phaseで「ホスト上の純粋ロジック試験」「RV32ECゲスト実行」「実機」を別々に記録する。PASS/PARTIAL/FAIL/UNVERIFIEDを保持する。

|Phase|必須の試験・成果|
|---|---|
|0|既存の32回切替、RAM/UART、SysTick、復元欠陥の負例を維持|
|1|片方yield、片方終了、両方終了、自分だけREADY、タスクreturn、選択禁止状態への切替なし|
|2|全番号、未知番号、不正pointer/overflow、ecallのPC前進、同一frameでの復帰、保存レジスタ|
|3|tick wrap、sleep(0)、同時timeout/event、通知直前/直後のBLOCKED、メッセージ満杯と再開|
|4|RAM/Flash超過リンク拒否、canary故障注入、最大call chain、panic/illegal trap診断|
|5|J4M6ピン検査、SWIO保持、software SPI read/write、TX busy、RX ring wrap/release、NIC reset|
|6|ARP応答、IPv4/ICMP checksumと奇数長、不正長drop、Macの実pingとパケット記録|
|7|handshake、部分送受信、データ/ACK/SYN/FIN損失、重複、順不同、seq wrap、窓0と回復、再接続|
|8|レジスタsentinelを保持した強制切替、syscall中tick、SPI/共有状態非再入、wait/wake競合|
|9|HTTP Echo: POST /echo、0〜64byte、過大本文拒否、分割受信、timeout、Macから本文一致・再接続|
|10|TCPシェル: ps/mem/uptime/netstat、公開snapshot API、長い行、不完全受信、backpressure、切断後再接続|
|11|J4M6で24時間以上の通信、1万回の接続/切断、NICリンク断/復帰、最大スタックとサイズ報告|
|12|同一の最終ELFで全機能、2KB/16KB、再現可能なビルドと実機配線、ABI/制限の仕様固定|

### 仮想EthernetとMac接続のゲート

Renvoに未実装のコマンドを前提にしない。Phase 6の前にvirtual NetworkDeviceと、フレームの双方向転送手段を実装する。最初はホスト側の決定論的フレーム注入／取得でARP/ICMP/TCPを試験する。これだけではMacのping/TCP接続成功とは呼ばない。

次に、Renvo側の仮想NICをMacのネットワークへ接続するhost adapterを実装する。macOS vmnet等を候補とするが、利用可能API、署名・権限、hostとの疎通は先に小さな実験で確定する。権限を要する構成は実施前に確認する。Linux TAP用手順をmacOSへそのまま持ち込まない。host adapterはEthernetフレームのみを中継し、ARP/ICMP/TCPに代理応答しない。

仮想ネットワーク接続が成立しない間も、フレーム注入試験でprotocol開発は進められるが、Phase 6はPARTIALに留める。ENC28J60の電気的・SPIタイミングの検証はvirtual NICで代替しない。

## 13. Phase 1の実装単位

1. Phase 0検証を残したまま、TaskId、TCB、TaskStateを導入する。
2. `select_next()` をREADY走査へ置換し、現在の固定xorを廃止する。
3. return trampolineとtask_exit、全終了／全待機の動作を追加する。
4. TCBを直接変更しない公開task APIとread-only snapshotを作る。
5. 片方終了後も他方のカウンタとstack/regsが保たれるRenvo試験を追加する。
6. 並行してJ4M6のピン／software SPIゲートを検証し、ボード設定を固定する。

1〜5は実装済み。実測・再現は [Phase 1検証](../ch32v003-rust-os-test/docs/scheduler.md) を参照。
6の物理ボード検証は未実施であり、Phase 5の実機ゲートとして残す。
Phase 1にTCPパーサやecallの本実装を混ぜず、次にPhase 2へ進む。

## 14. 未確定事項と決定期限

- J4M6の共通パッド初期化、SWIOとの共存、software SPI性能: 基板設計／Phase 5前。
- Renvo virtual NICの実装位置とMac host adapter・権限: Phase 6開始前。
- 80byte trap frameと256byte kernel stackの実機成立性: Phase 2で測定、Phase 4で予算固定。
- MSS=128、tick=10ms、RTO上限、TIME_WAIT期間、ISN生成、未対応IP機能: Phase 7で相互運用と適合表を作り固定。
- RAM予備256byte、Flash予備1KBの実残量: 各Phaseで計測。超過する場合はバッファ／診断／実装を再設計し、最終容量の条件は維持する。

これらは実装開始を妨げる未回答質問ではなく、後段Phaseに入るための検証ゲートである。
