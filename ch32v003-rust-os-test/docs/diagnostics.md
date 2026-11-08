# Phase 4: 静的メモリ・監視・障害診断

Phase 3の2タスク・syscall・timer・IPCと同じ実行経路へ監視を追加した。
追加feature `diagnostics` は `services` を含み、各タスク256byte、起動/IRQ/kernel共用256byte。
ヒープや動的なタスク/待機領域は使わない。

## スタックと公開snapshot

`rv2k-arch::stack::StackRegion`はvolatile読取りで番兵とwatermarkを検査する。
全スタックの下端16byteはguard。最初の1語だけでなく4語すべてを検査する。
ecallの保存前、IRQ、タスク選択時にguardを確認し、破損時は復帰せず診断へ進む。
元spのalignmentと80byteフレームの上下限も保存前に検査する。
これは故障検出であり、MMUによる保護や任意のメモリ破壊の封じ込めではない。

`rv2k_api::mem::info()`は番号13からversion=1の24byte snapshotを取得する。
静的RAM、スタック予約、watermark、guard bitset、Flash load span、容量上限を含む。
出力先は従来と同じポインタ検査を通り、kernelの構造体への参照は渡さない。
TaskInfoと同様、versionと固定長を持ち、カーネル内でformatしない。

## 障害経路

`FAULT_INFO`は24byteの固定記録: reason、task、pc、cause、tval、元sp。
タスク例外はトラップフレーム、kernel内例外はCSRとasmで保存した元kernel spから取得する。
panicはreasonを記録し、PC/cause/tvalは0（panic発生箇所の正確なPCとは呼ばない）。
障害時はMIEを禁止し、kernel stackを巻き戻してから診断記録とUARTの短いhex出力を行う。
UARTは1byteごと最大64回のready検査で打ち切り、応答不能でも無限待機しない。
Renvoでは最後のebreakを期待停止として扱う。実機は停止ループへ進む。

## 不正命令をゲストへ渡すRenvoパッチ

固定コミットのCPUは通常のdecode失敗をホストのCpuFaultで止める。
`patches/renvo-illegal-trap.patch`は、QingKeゲストが非resetのmtvecを設定した場合に限り、
illegal instructionをcause=2としてvectorし、命令をmtvalへ記録する。
未設定時のデバッガ向け停止動作は維持する。これは実機検証の代わりではない。
`setup-emulator.sh`で固定HEADを検査し、パッチを一度適用（適用済みならreverse-check）してビルドする。
追加した不正32bit命令・RV32E高位レジスタ・不正16bit命令のCPU試験を含め、29 CPU試験が通った。

## 容量検査

`memory.x`はRAMの静的領域とkernel stackの衝突、Flashのロード上限、kernel stack予約の不正値を拒否する。
`check-budget.py`はデバッグ情報を含むELFファイル長ではなく、元の実機ELFのPT_LOAD物理範囲を検査する。
.dataのFlash初期値とalignmentを含め、SRAMの静的領域/タスクスタックとkernel予約を別途加算する。
RV32EC/ILP32E flagsとallocatorシンボル不在も確認する。
`budget-negative.py`は実際に同じリンカスクリプトを使い、静的RAM1792+stack256の境界成功、
RAM1793+stack256の拒否、Flash16385byte以上の拒否を検証する（ローダーの拒否だけではない）。

## 再現と実測

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cd ch32v003-rust-os-test
bash scripts/setup-emulator.sh
bash scripts/diagnostics.sh
python3 scripts/diagnostics-negative.py
python3 scripts/budget-negative.py
```

[実測](logs/diagnostics-summary.json): 1724937命令、30 syscall、6 timer IRQ。
公開mem APIと64byte IPC/空メッセージ/4byte応答、全待機・復帰を同じビルドで実行した。
今回実行したcall chainのwatermarkはタスク各184/256byte、kernel204/256byte。
[元ELFの容量](logs/diagnostics-budget.json)はFlash8200/16384byte、静的RAM768byte＋kernel256byte＝1024/2048byte。
将来のdriver/TCP追加後の上限や未実行のcall chainを保証する数値ではなく、追加ごとに再測定する。

[7種類の障害注入](logs/diagnostics-negative-summary.json)で、タスクguardの第3語、kernel guardの第3語、
panic、タスクの不正命令、kernel内不正命令、sp=0、UART応答不能を確認した。
sp=0検出後にタスクスタックへstoreがないこともbus traceで検査する。
UART不能はvolatileなテスト状態を読み、ready待ち上限を実行したうえで22101命令以内に停止した。
[リンク境界試験](logs/budget-negative-summary.json)と[Phase 0〜3回帰](logs/phase4-regression.json)も保存した。
大きいbus/coverageは再現時生成とし、Gitにはsummary・実行結果・サイズ等を残す。

CIでもCPUパッチ試験、正常/障害注入、実リンカ境界、従来回帰を実行する。
CH32V003実機での最大スタック、PFIC/HPE構成、UART故障時挙動は未検証であり、実機ゲートとして残る。
