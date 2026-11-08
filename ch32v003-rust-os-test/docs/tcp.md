# Phase 7: TCP / Socket API

状態: **実装中**。TCP wireと公開socket syscallをRV32ECの固定2タスクへ接続した。
ホスト55試験、合成packet peerとのguest echo・再送・再接続はPASS。
仮想NIC構成はRAM 1,808byte / Flash 15,320byte。
**ENC28J60構成はFlashが1,808byte超過してlink FAIL。Phase 7は未完了で、マージ前に修正が必要。**
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
firmwareはsocket/event呼出しをこのhookへ渡し、非対象の呼出しだけを既存dispatcherへ渡す。同じevent waitを二度処理しない。アプリの静的32byte領域だけをT1の許可範囲へ追加する。

## 検証

```sh
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable test --workspace --all-features --locked
```

全55ホスト試験（基礎50＋wire/link 4＋Clockの広域比較1）がPASS。
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

## wire / guestの検証

`bash scripts/tcp.sh` はcompact memory probe、guest TCP、2構成の容量を検査する。
現在は最後のENC容量検査でFAILする。上限を緩めてPASSとして扱わない。
ビルドは固定nightly、opt-level=z、target-feature=+relax、LLD ICF/relaxを使う。
relaxはRust上でunstable feature warningが出るためtoolchain pinとRV32EC実行試験を維持する。
メモリ操作はRV32Eの短いbyte loopに置き換え、専用probeでunaligned、重なり両方向、同一領域、長さ0、戻り値、memsetの下位byteを実命令で検査する。
Clockは32bit除算とoverflowしないremainder加算を使い、6種のperiod・各1万回をu64参照演算と比較する。
protocol/socket領域はBSSへ置き、起動時に小さな設定だけを書いて初期データのFlash消費を減らす。

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

実測: 5,374,991 instructions、RX25 / TX31、echo193byte、listen3回、syscall167回、timer IRQ25回、uptime6522tick。
T0/T1/kernel stack watermark=232/120/168byte、全番兵正常、FAULT_INFO=0、IO error=0。
PCAPとSRAMを独立Python oracleでも照合する。
原本: `docs/logs/tcp-{run,summary,budget}.json`、`tcp-output.pcap`、`tcp-output.rx.pcap`、
`compact-{run,summary,budget}.json`。ENCの失敗は `tcp-enc-link-failure.json` に別記する。

残る必須作業はENC構成の16KB収容、追加のwire損失・異常終了試験と適合表、Mac native TCP検証。
実機のstack/SPI timingは未検証。Phase 8ではprotocol/NIC借用中のpreemption延期とcontext保存を含めて再計測する。

設計根拠: [RFC 9293](https://www.rfc-editor.org/rfc/rfc9293.html)、
[RFC 6298](https://www.rfc-editor.org/rfc/rfc6298.html)、
[RFC 9006](https://www.rfc-editor.org/rfc/rfc9006.html)、
[RFC 1337](https://www.rfc-editor.org/rfc/rfc1337.html)。
全TCP仕様への適合は未検証。縮小profileの適合・省略事項はwire実装と合わせて記録する。
