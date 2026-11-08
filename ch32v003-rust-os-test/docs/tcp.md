# Phase 7: TCP / Socket API

状態: **実装中**。今回の変更はTCP状態機械、固定socket領域、syscall境界の基礎実装。
ホスト上で検証しているが、EthernetのTCP parser/encoderとfirmwareのdispatchへはまだ接続していない。
現行ゲストのsyscall 7..10は引き続きNoSysであり、Phase 7の完了やMac TCP通信成功を意味しない。

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
実際の起動seed・quiet timeとその運用は、firmwareへ接続する変更で確定・検証する。

`rv2k-api::socket` は既存番号7..10のecall wrapperを提供する。
`rv2k-kernel::socket::dispatch` はT1の所有権、port範囲、ユーザーコピー、
level readinessを含むevent waitを処理する。公開syscallの呼出し後にuser pointerを保持しない。
成功した操作でT0へDRIVER_POLLを通知し、T0終了後のsocket操作はClosedにする。
これはfirmwareから既存service dispatcherの前に呼ぶためのhookであり、今回はまだ有効化しない。

## 検証

```sh
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets -- -D warnings
cargo +stable test --workspace --locked
```

全50ホスト試験（Phase 6時点37＋今回13）がPASS。
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

TCPのpacket parser、pseudo-header checksum、wire image、未知options、損失注入harness、
RV32EC上のsocket syscall、stack/Flash/RAM、Mac相互運用、適合表は後続の必須作業。
ここでのmetadata単体試験をその代わりとしない。
既存のPhase 6 guest構成は維持し、このTCP実装を有効化していない。

設計根拠: [RFC 9293](https://www.rfc-editor.org/rfc/rfc9293.html)、
[RFC 6298](https://www.rfc-editor.org/rfc/rfc6298.html)、
[RFC 9006](https://www.rfc-editor.org/rfc/rfc9006.html)、
[RFC 1337](https://www.rfc-editor.org/rfc/rfc1337.html)。
全TCP仕様への適合は未検証。縮小profileの適合・省略事項はwire実装と合わせて記録する。
