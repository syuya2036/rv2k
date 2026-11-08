# Phase 7: TCP / Socket API

状態: **PARTIAL**。TCP wireと公開socket syscallをRV32ECの固定2タスクへ接続した。
ホスト59試験、合成packet peerとのguest echo・再送・再接続はPASS。
仮想NIC構成は予約RAM 1,824byte / Flash 13,852byte。
ENC28J60構成は予約RAM 1,856byte / Flash 16,180byteで、16KB/2KBのstrict linker gateをPASS。
ソフトウェア・容量のゲートはPASS。Mac native TCPと実機のゲートを残す。
以下の実測はPhase 7当時の記録。現在のIRQ統合・容量・stackは[Phase 8検証](preemption.md)を参照。
Macのnative TCP、J4M6実機はUNVERIFIED。合成peerの成功をMacからの実通信として数えない。

## 実装済みの基礎

`rv2k-net::tcp::Tcp` は単一接続・受動openの状態を保持する。
LISTEN、SYN_RCVD、ESTABLISHED、FIN_WAIT_1/2、CLOSING、CLOSE_WAIT、LAST_ACK、TIME_WAITを区別する。
`Segment` は上位parserで長さ・checksumを検証済みという契約で、payloadはcallbackで必要範囲だけコピーする。
受信のためのcallbackが失敗したとき、未完成のpayloadを公開せず、RCV.NXTを進めない。
送信候補の取得とNIC送信完了を分け、TX busy/IO失敗でSND.NXTや再送期限を先に進めない。

`Socket` はRX128byte、未ACK/未送信TX128byteを所有する。アプリは部分コピーとlisten/close要求だけを行う。
送信を受理するとbyte列をACKまで保持し、部分ACKで未ACK suffixだけを残す。
RXはin-order suffixだけを受理し、重複先頭を除去、順不同は保管せず累積ACKを返す。
最後に広告した受信窓の右端で受理範囲を制限し、アプリが消費するとwindow updateを要求する。
EOFはbufferを読み終えてから0を返す。close後のsend、古いhandle、timeoutを区別する。
終端時に世代を更新し、次のlistenは新しいhandleを返す。

RTOは10ms tick、初期1秒、SRTT/RTTVARの整数演算、最大60秒の指数backoff。
再送済みflightからRTT sampleを取らない。SYN再送後は有効sampleがなければ最低3秒にする。
通常再送は8回まで。窓0のpersistは別期限で続け、窓が開くまで通常再送回数で切断しない。
利用可能なACK/FINを処理してから最終timeoutを判定する。
受信窓内でもRCV.NXTと一致しないRSTはchallenge ACK、TIME_WAIT内RSTは無視する。
TIME_WAITは120秒、FIN_WAIT_2の待機上限は60秒。slotを解放するまで別接続へ再利用しない。

ISNはtuple・時刻と接続ごとの進行値を使い、固定0にはしない。暗号学的な予測困難性はない。
再起動時に永続的なsequence進行を保証できないcallerは、constructorへ明示的なquiet deadlineを渡す。
一度quiet timeを満了した後はu32 tick wrapで再びquietへ戻らない。
firmwareは起動seed 0x10203040、1 MSL=60秒のquiet timeを使う。合成peer試験は最初のIRQ・listenを待ってRenvoのperipheral timeを60秒進め、guest自身のClock/SysTick処理でquiet timeを満了させる。TCP stateやsequenceをホストから書き換えない。実機・通常のUDP bridgeではこの時間短縮を行わない。

`rv2k-api::socket` は既存番号7..10のecall wrapperを提供する。
`rv2k-kernel::socket::dispatch` はT1の所有権、port範囲、ユーザーコピー、
level readinessを含むevent waitを処理する。公開syscallの呼出し後にuser pointerを保持しない。
成功した操作でT0へDRIVER_POLLを通知し、T0終了後のsocket操作はClosedにする。
firmwareは全サービスを統合dispatcherで一度だけdecodeし、socket・IPCのlevel readinessを同じevent waitへ渡す。既存の個別dispatcherは単独ABI試験用に残す。アプリの静的32byte領域だけをT1の許可範囲へ追加する。

## 検証

```sh
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable test --workspace --all-features --locked
```

全59ホスト試験（基礎50＋wire/link 6＋Clockの広域比較1＋統合dispatcher/lazy snapshot 2）がPASS。
今回の試験範囲:

- SYN損失・重複SYN・MSS・quiet time、SYNへのwindow 0。
- RXの重複・重なり・順不同・seq wrap・満杯・消費後のwindow update。
- 部分ACK・送信suffix再送・未送信範囲へのACK拒否・別peer拒否・TX seq/ACK wrap。
- NIC送信前の状態不変と、RX/ユーザーコピー失敗時の非公開。
- persist probeを12回行っても窓0で生存し、窓再開後に元のbyte列を送信。
- EOF前の受信byteの読出し、FIN損失・再送、passive/active/simultaneous close。
- TIME_WAIT内の重複FINへの再ACKと期限更新、RSTからの保護、slot再利用制限。
- RST sequence検査、8回の再送backoff、timeout、期限wrap、最終timeoutと同時のACK。
- syscallの所有権・部分コピー・不正pointer・level wait・ネットワークタスク終了。
- wire経由のRST challenge/正確なRST、SYN再送上限・Timeout通知・handle更新後の再listen。
- 統合dispatcherでのsocket/IPC readiness、公開mem snapshotの遅延取得。

## wire / guestの検証

`bash scripts/tcp.sh` はcompact memory probe、guest TCP、2構成の容量を検査する。
仮想NICとENCの両構成を、16,384byte Flash / 2,048byte RAMの上限で検査する。
ビルドは固定nightly、opt-level=z、target-feature=+relax、LLD ICF/relax/relax-gpを使う。
relaxはRust上でunstable feature warningが出るためtoolchain pinとRV32EC実行試験を維持する。
メモリ操作はRV32Eの短いbyte loopに置き換え、専用probeでunaligned、重なり両方向、同一領域、長さ0、戻り値、memsetの下位byteを実命令で検査する。
Clockは32bit除算とoverflowしないremainder加算を使い、6種のperiod・各1万回をu64参照演算と比較する。
protocol/socket、scheduler、IPC、空のfault recordはBSSへ置き、起動時に小さな設定だけを書いて初期データのFlash消費を減らす。
空のfault recordはreason=0で、他のfieldはreasonが非0のときだけ意味を持つ。
mem snapshotはMemInfo syscallで必要なときにだけ計算し、little-endianではpaddingのない24byte構造体を直接copy-outする。

`__global_pointer$`をSRAM末尾に定義し、起動、trap入口、nested fault入口、fault reporterでnorelaxの`la gp`を実行する。タスクの初回frameにも同じ値を設定する。Rust実行中はこのgpを保ち、trap frameはタスクのgp値を保存・復元する。emulator-stop構成のGPを0x55へ変更したassembly probeで実timer IRQ 2回以上とGetPidを実行し、Rustへ戻る前に元のgpを復元する。TCP checkerはprobe成功と公開mem snapshotをSRAMから確認する。
`diagnostics-negative.py --tcp-build-profile`はTCPと同じ最適化設定で9種類の実guest faultを検査する。標準profileの負例、syscall/coop復元欠陥、リンカ境界も再検証した。linker script変更で必ず再リンクするためbuild.rsでmemory.xを監視する。

TCP parserはIP optionsとTCP optionsを同時に保持せずheader scratchを再利用し、header 96byte＋stream 64byteに小さなpacket metadataを付ける。
IPv4、TCP header/checksum、受信状態更新を別々の有限段階にし、途中でyieldせず最後にRXを一度releaseする。
MSS/NOP/EOLと未知optionの長さを検査し、最大IP40＋TCP40byte options、奇数長、受信窓を越えるpayloadの後端checksum破壊をhost試験で確認する。
データは全体checksumの検証後、受理する範囲だけをRX128byteへ読む。
送信にはpseudo-header checksumを付け、NIC commit成功後だけprotocol stateを更新する。

`host/net-harness` のtcp modeは独立した合成peer。rv2k-netを使わずにpacket生成・checksum・echo byte列を検査する。
SYN応答1個、データACK1個、FIN ACK1個を失わせ、再送を確認する。
順不同・重複入力、peer MSS=24、sequence wrap、65/128byteの2接続を扱う。
アプリは公開listen/recv/send/closeとevent waitだけでechoし、EOF後にcloseして再listenする。
待受け中は接続イベントを無期限に待ち、接続後のデータ待ちだけを有限timeoutにする。
ホストの時間前進は再送期限用に計65秒。MMIO deviceは時間経過を反映し、IRQ/Clockはguestが処理する。

実測: 6,491,124 instructions、RX25 / TX31、echo193byte、listen3回、syscall177回、timer IRQ29回、uptime6527tick。
T0/T1/kernel stack watermark=236/156/164byte、全番兵正常、FAULT_INFO=0、IO error=0。
PCAPとSRAMを独立Python oracleでも照合する。
原本: `docs/logs/tcp-{run,summary,budget}.json`、`tcp-output.pcap`、`tcp-output.rx.pcap`、
`compact-{run,summary,budget}.json`、`tcp-enc-budget.json`。
過去フェーズのrawログは当時の実測として残し、今回の再検証結果は`tcp-regression.json`へ記録する。

残るPhase 7ゲートはMac native TCP検証。host adapter経路は未検証で、合成peerによるwire検証とは区別する。
実機のstack/SPI timingは未検証。Phase 8ではprotocol/NIC借用中のpreemption延期とcontext保存を含め、仮想HWで再計測した。

設計根拠: [RFC 9293](https://www.rfc-editor.org/rfc/rfc9293.html)、
[RFC 6298](https://www.rfc-editor.org/rfc/rfc6298.html)、
[RFC 9006](https://www.rfc-editor.org/rfc/rfc9006.html)、
[RFC 1337](https://www.rfc-editor.org/rfc/rfc1337.html)。
## 縮小profileと検証範囲

全TCP仕様への適合を宣言するものではない。以下は実装契約と検証の対応表。

|領域|実装・制限|検証|
|---|---|---|
|接続|単一slot、IPv4、同一LAN、passive open。active/simultaneous openは未対応|host state/wire、guestで2接続と3回listen|
|転送・整列|RX/TX各128byte、未ACK flight 1個、peer MSS/windowで制限。順不同は捨て、重複prefixを除去|host wrap/partial ACK/full window、guest MSS24・重複・順不同|
|checksum/options|IP/TCP全体のchecksum、TLVの長さ検査、MSS/NOP/EOL。未知optionは妥当な長さでskip|最大IP/TCP options、奇数payload、受信窓より後方の破損、異常TLV|
|flow control|広告windowの右端を追跡。window 0では別persist期限、通常retry上限で切らない|host 12 persist回とwindow再開、未送信ACK拒否|
|再送・輻輳|1 MSS以下のflightを維持。整数SRTT/RTTVAR、Karn、指数backoff、通常retry 8回。fast retransmit/大きなcwndは実装しない|host estimator/timeout、wire SYN retry上限、guest SYN/data/FIN損失|
|切断|passive/active/simultaneous close、EOF、FIN_WAIT_2上限、TIME_WAIT 120秒。RSTはsequence検査、TIME_WAITでは無視|host close状態遷移、wire RST、guest FIN再送・再接続|
|ISN/再起動|tuple・時刻・接続counterと起動quiet time。暗号学的なISN予測困難性はない|host quiet/wrap、guestが60秒経過を処理してからhandshake|
|API/所有権|T1所有のopaque handle。コピー完了まで非公開、部分send/recv、level event。NIC TX commit後だけ送信状態更新|host copy fault/busy/handle失効、wire TX IO、guest syscall・mem snapshot|
|未対応・適合上の制限|URG/urgent data、SYN payloadの先行受理、TCP keepalive、SACK、window scale、timestamps、ECN negotiation、IP fragments/ICMP errors、複数接続、認証/TLSは対象外|これらを完全対応と扱わない。一般TCP全仕様への適合・外部TCP suiteは未検証|
|外部ゲート|Mac native TCP、実機SPI timing・実機stack、長時間運転は未確認|Phase 5/6/7のPARTIALを維持|

Phase 8以降の追加コードで容量とstackを再検査する。現在のENC Flash余裕は204byte、guest T0 stack余裕は番兵を除いて4byteであり、後続機能の収容を保証しない。
