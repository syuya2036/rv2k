# Phase 8: timer preemption

状態: **ソフトウェアゲートPASS**。固定2タスクのIRQ強制切替、非再入カーネル、共有状態の保護をRenvoの実RV32EC命令で確認した。Mac native通信とJ4M6実機はUNVERIFIED。

## 切替と共有状態

SysTickは10msごとにClockを更新し、peerがREADYなら切り替える。実行可能なpeerがいない場合とidleへのIRQは元の一時frameから復帰する。IRQはmepcを進めない。ecallは従来どおり4進める。

IRQ入口はkernel stack上に80byteの一時frameを作る。切替可能な場合だけ元のtask spを検査してtask stackへframeをコピーし、一時frameを解消してからschedulerを呼ぶ。カーネルのRust call stackを残して別タスクへ入らず、非ネストIRQ・MIE禁止・mscratchによる同期例外検出を維持する。

`rv2k_arch::preempt::disable()`はネスト可能なRAII guard。入口・出口のcounter更新だけを短いcritical区間にし、本体のIRQは禁止しない。IRQ要求はcoalesceして保留する。最後のguardを解放した後、checkpointまたはinterrupt-enabledなsyscall完了境界で切り替える。外側のcritical区間でMIEが禁止されている間は、guardを解放してもpending要求を消費しない。checkpointとGetPidもこの除外を維持する。

ガード中または呼出し元のMIEが禁止されている間、yield/sleep/event_wait/task_exitはBusy。待機情報や終了状態を書き込む前に拒否する。通常のcopy・非ブロッキングサービスはそのまま使える。

T0はprotocol/NIC/scratchを借用する前にguardを取得し、pollとsocket event取得を完了してから解放する。event bitsはu32へコピーし、共有参照を残さずcheckpoint・signal・waitへ進む。公開uptime APIの8byte一時領域は独立した関数で使い終え、深いparser/driver chainへstack frameを重ねない。

## 再現

```sh
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable test --workspace --all-features --locked
cd ch32v003-rust-os-test
bash scripts/setup-emulator.sh
bash scripts/preempt.sh
python3 scripts/diagnostics-negative.py --preempt-build-profile
```

`preempt`は通常のサービスまたはTCPと組み合わせる。最大除外 clocks の計測は `preempt-timing` で追加する。`preempt-probe` と snapshot probe、および `preempt.sh` の仮想 TCP はこの計測を有効にし、通常の ENC / shell は無効にする。深さ・pending・IRQ 延期・切替動作はどちらにも残る。`preempt-probe`は専用2タスクとGPIO/SPIの所有者であり、net/driversとは同時に有効化しない。

TCP同様にnightly-2026-03-12、RV32EC/ILP32E、opt-level=z、+relax、LLD ICF/relax-gpを使用する。追加のLLVM設定`-riscv-enable-global-merge=false`は、独立したSRAM globalへのGP relaxationを有効に使うためのもの。内部compiler optionなのでtoolchain pin、両構成のELF容量検査、実命令回帰を維持する。

AppのGP/mem probeとAPP_BYTES/APP_LISTENSはemulator-stop構成の計測専用。通常のENC構成でもmem/socket/IPC等の公開サービスは残る。障害表示は保存済みFaultInfoを直接読み、6wordの余分なstackコピーを作らない。fault試験はUARTの全6fieldとSRAMを照合する。

## 検証結果

|項目|結果|
|---|---|
|workspace host tests / NIC contract|62 PASS / 1 PASS|
|CPU-only probe|12,338,386 instructions、3 roundずつ完了|
|forced switches / timer IRQ|47 / 51|
|非yield loop内のIRQ trace|T0=3、T1=4（先頭8切替の記録）|
|deferred要求のsyscall完了境界切替|1（GetPidの戻り値・ecall PCも検査）|
|nested guard中のIRQ / 禁止呼出し拒否|5 / 8|
|実software SPIへのIRQ|CSを保持し、SCK立上り3個目の後にIRQ。MOSI A5 3C、MISO 0、16bitを照合|
|CPU probe T0/T1/kernel stack|208 / 208 / 164byte、全番兵正常|
|通常TCP / NIC読出し中IRQ注入|2接続・193byte echo、両方PASS|
|NIC途中IRQ|入力25frameすべてで1tickを前進、26回延期、3回強制切替|
|TCP T0/T1/kernel stack|220 / 152 / 164byte、全番兵正常|
|virtual NIC reserved RAM / Flash|1,836 / 14,144byte|
|ENC28J60 reserved RAM / Flash|1,844 / 16,380byte、strict gate PASS|

通常TCPのguard最大時間は7,929 clocks（24MHzで約0.33ms）。NIC IRQ注入では各frameの読出し中に240,000 clocksを意図的に追加し、最大248,376 clocksを測定した。CPU probeの963,841 clocksは、ネストした除外中に4tick待つ専用fixtureの値。これらを実機のSPI時間・最大遅延として扱わない。

CPU probeはgp/tp/ra、temporary、saved、argument registersをsaltで識別する。a5の非yield loop counterとa4の実行数を対応させ、IRQでPCを進める欠陥も検出する。外側critical区間でguardを解放し、checkpointとGetPidを呼んでもpeerが開始しないこと、MIE復元後にpending切替が起こることも検査する。IRQ禁止中の2tick経過は失われず、uptimeへ反映される。

負例はmissing-t1 restore、IRQ PC+4、guard無視の3ケースで、すべて実guest panicを検出する。さらにpreempt profileの9障害注入（番兵・invalid sp・panic・illegal instruction・kernel trap・stalled UART等）がPASS。通常/TCP profileの障害注入と、既存の全Phase 0〜7回帰もPASS。

ホストのtcp-preempt modeは最初のRX byteを観測し、guestのguard depthが非0であることを読んでからperipheral timeだけを1tick進める。CPU register、protocol state、interrupt flagは書き換えない。PCAP checksum・再送byte列・RX release・socket syscall・番兵は独立oracleで検査する。

原本は`docs/logs/preemption/`のprobe memory/trace、実行結果、PCAP、summary、budget、negative、regression。大きなbus logはスクリプトで再生成する。過去Phaseのrawログは当時の実測として残す。

ENC Flashの余裕は4byte。後続のHTTP/shellを収容できる保証はなく、機能追加後に同じ上限で再検証する。実機のPFIC/HPE設定、SPI電気特性・実機stack、長時間運転は未確認。実機がないため、仮想HWの成功から実機PASSを推定しない。

Phase 10 の共通最適化後の測定は [shell の回帰記録](logs/shell/regression) を参照。既存の表・raw log は Phase 8 時点の記録として残す。専用 safe-point probe は、4 回の保護中 IRQ を観測した後、MIE 復元前に次の周期 IRQ を 1 tick 先へ再設定する。未処理の延期要求が GetPid 境界に届くことを、コンパイラによる命令時間差から独立させて検査する。
