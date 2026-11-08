# Phase 6: virtual Ethernet / ARP / IPv4 / ICMP

状態: **PARTIAL**。protocolのホスト試験とRV32EC実命令・フレーム注入はPASS。
Macからの実pingとhost adapter経路はUNVERIFIED。J4M6＋ENC28J60は未保有。
フレーム注入を実pingや実機検証として数えない。

`rv2k-net` はno_std / heapなしの自作protocol core。
固定MAC `02:00:00:00:00:01`、IP `192.168.123.50/24` を使う。
フレームはNICに置き、MCUにはheader 96byteとstream 64byte、ARP cache 1件、
保留ARP応答1件、ARP解決要求1件と統計だけを保持する。
送信途中のエラーではabortし、受信は成功・不正・送信失敗いずれも一度releaseする。

ARPはEthernet/IPv4・固定長・送信元MAC一致・対象IPを検査して応答する。
同一サブネットのunicast peerだけをcacheし、30秒で失効する。
解決要求は1秒間隔で最大3回、wrapを考慮して再試行する。
TX busy時のARP応答はpeer情報だけを保留して再試行する。

IPv4はversion、IHL 20..60、total length、TTL、宛先、checksum、option長を検査する。
fragment、reserved flag、source routeは破棄する。再構成・ルーティングは行わない。
ICMP Echoはcode 0と全体checksumを検査し、NICから64byteずつ再読出しして応答する。
IP optionsは検査後に除去し、新しい20byte IPv4 headerを生成する。
ICMP送信時にTX busyなら破棄する（IPは信頼性を提供しない）。TCPはPhase 7で追加する。

## 実行経路と再現

```sh
cargo +stable test --workspace --locked
cd ch32v003-rust-os-test
bash scripts/setup-emulator.sh
bash scripts/network.sh
```

Renvoの固定revisionへ外部MMIO deviceを登録する小さなpatchを適用し、
`host/net-harness` が `0x50000000..0x50001000` に仮想NICを接続する。
RX ringは6144byte、単一TXは最大1514byte、短い送信は60byteにpadする。
ホストdeviceはframe格納とstream入出力だけを実装し、ARP/IP/ICMPの代理応答はしない。
PCAP入力はlittle-endian Ethernet、FCSなし、非truncate、最大1514byteに限定する。

ゲストは既存の固定2タスク・各256byteスタック・syscall gateで起動する。
T0がprotocolをpollし、公開event wait（timeout 1 tick）で待機する。
T1は公開event APIで完了通知を待つ。SysTick IRQとidleからの復帰も実際に通る。
再生が終了するとT0がT1へ通知して両タスクが終了する。
診断用host-stop registerはエミュレータだけに存在し、実機構成は継続pollする。

Pythonの独立checksum・packet oracleが入力と応答を比較し、SRAM snapshotから
正常終了、FAULT_INFO、全スタックの番兵、watermark、protocol counter、RX release数を検査する。
haltだけでは成功とせず、診断faultでのEBREAKも失敗にする。

## 計測結果

|項目|結果|
|---|---|
|workspace host tests|37 PASS（うちprotocol 6）|
|virtual driver ↔ host model contract test|1 PASS|
|RV32EC instructions|5,059,017|
|注入 / 正常応答 / 不正破棄|21 / 12 / 9|
|checksum error / fragment / IO error|2 / 1 / 0|
|syscall / timer IRQ|49 / 20|
|T0 / T1 / kernel stack watermark|204 / 120 / 204 byte、全番兵正常|
|virtual NIC RAM / Flash|1,280 / 11,636 byte|
|ENC28J60構成 RAM / Flash（linkのみ）|1,328 / 16,108 byte|

RAMには静的領域、2タスク分512byteとkernel/IRQ/boot stack 256byteを含む。
NIC frame memoryは別資源。物理ENC構成の実行時stack使用量は未検証。
IPv4とvirtual IOをinlineし、pollを独立関数にすることで、当初検出したtask stack破壊を解消した。
Phase 8のpreemptionでは追加のcontext保存を含め、改めてstackを測定する必要がある。
ENC構成のFlash残量は276byteであり、TCP等を足す際はコードサイズ削減と再計測が必要。

PCAPはARPとICMP payload 0/1/55/56/57/63/64/65/1472byte、4/40byte options、
不正MAC・IHL・TTL・checksum・fragment・option・truncateを含む。
ホスト試験ではARP busy、cache失効、時刻wrap、解決再送・応答による停止、
TX失敗時abortとRX releaseも確認する。
原本は `docs/logs/network-{input,output}.pcap`、`network-run.json`、
`network-summary.json`、`network-budget.json`、`network-enc-budget.json`。

## Mac host adapter gate

`host/vmnet/bridge.c` はvmnet host modeとloopback UDPの間でEthernetだけを中継する。
ビルドと通常権限での起動を試し、`vmnet_status=1001`（一般エラー）で失敗した。
これはpermission denied固有のcodeではなく、権限不足が確定したとは扱わない。
設定・終了処理・実pingの手順は [host adapter](../../../host/vmnet/README.md) を参照。
権限を要する実行は設計書の事前確認ゲートに従う。成功ログが揃うまでPhase 6はPARTIAL。
