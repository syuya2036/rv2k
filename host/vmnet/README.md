# macOS vmnet Ethernet adapter

`bridge.c` はApple SDKのvmnet APIでhost mode networkを作り、EthernetをUDP経由で
Renvoの仮想NICへ渡す。パケット内容の書換え・ARP/ICMP/TCPへの代理応答は行わない。
固定network UUID `5f90c6e8-2cd1-4daf-b152-d2157222a32c`、Mac `192.168.123.1/24`、
guest `192.168.123.50`。DHCPなし。TSO/checksum offloadを無効にし、guestがchecksumを計算する。
UDPは `127.0.0.1:44001`（bridge）↔ `127.0.0.1:44000`（harness）。

```sh
bash host/vmnet/build.sh
host/vmnet/target/probe
```

このMacの通常実行ではprobeとbridgeがともにstatus 1001で失敗した。
そのためビルドのみ検証済み、host adapterの通信はUNVERIFIED。
SDKでは1001はVMNET_FAILURE（一般エラー）。特定のpermission deniedとは断定しない。
rootまたは適切なentitlementによる利用可否は別途検証する。

## 実pingの手順（事前の実行確認が必要）

まず `bash ch32v003-rust-os-test/scripts/network.sh` を実行してfirmware/harnessを作る。
未使用のsubnetであることを確認する。同じ時刻に複数bridgeを起動しない。
設計書の「権限を要する構成は実施前に確認する」に従い、利用者が許可した場合に限り
bridgeをroot等の必要な権限で起動する。harnessとpingは通常ユーザーで動かす。
以下は各ターミナルで順に起動する例。

```sh
# bridge: 60秒で自動停止。SIGINT/SIGTERMでもvmnet_stop_interfaceを呼ぶ。
sudo -n ./host/vmnet/target/bridge 60
```

```sh
cd ch32v003-rust-os-test
vendor/renvo_emu/target/release/rv2k-net-harness \
  target/network/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test.remu \
  127.0.0.1:44000 docs/logs/network-live.pcap docs/logs/network-live-run.json \
  udp 127.0.0.1:44001 30
```

```sh
ping -c 3 192.168.123.50
ping -c 3 -s 57 192.168.123.50
```

bridgeの起動成功だけを疎通成功としない。ping応答、`network-live.rx.pcap`（入力）、
`network-live.pcap`（guest応答）、bridge転送件数と終了結果、guestの正常終了・番兵を照合する。
UDP ring overflowはreportの `rx_overflow` に記録する。ログが揃うまでは実ping未検証のままにする。
bridgeは期間満了・signal・転送エラー時にvmnetを停止し、終了callback失敗も非ゼロで報告する。
start時のタイムアウトやSIGKILLの場合はプロセス終了による解放に依存する。
ネットワークが意図どおり解放されたかも実行時に確認する。

APIと権限の根拠: このMacのXcode SDK `vmnet.framework/Headers/vmnet.h`、
[Apple vmnet](https://developer.apple.com/documentation/vmnet)、
[networking entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.vm.networking)。
