# Phase 1: 固定2タスクの管理

`crates/rv2k-kernel` はMMIO非依存の `no_std` スケジューラ、
`crates/rv2k-api` はTaskId・TaskState・read-only snapshotを定義する。
TCBは非公開。READYだけを次スロットから巡回して選択し、RUNNINGは最大1個。
EXITEDスロットは再利用しない。全BLOCKEDはIdle、全EXITEDはFinishedを返す。

ホストからの再現（リポジトリルート）:

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
```

RV32EC実行（Phase 0と同じ固定nightly・Renvoセットアップが必要）:

```sh
cd ch32v003-rust-os-test
bash scripts/setup-emulator.sh
bash scripts/scheduler.sh
```

ゲストは各256byteの独立スタックと64byteの協調フレームを使用する。
asmは全レジスタを保存してから起動用スタックへ移り、Rustの選択処理を呼ぶ。
probeで変更されたgp/tpはRustへ入る前に起動時の0へ戻す。
タスク入口のtrampolineは関数returnをexit(0)に変換する。
Bは3ラウンド後にexit(-7)、Aは8ラウンドまで進んでreturnする。
B終了後のA自身への復帰でも、x3〜x15とスタックのsentinelを検査する。
終了処理は終了タスクのスタックを使用しない。

実測は [scheduler-summary.json](logs/scheduler-summary.json):
5952命令、23回の選択、カウンタ8/3、終了コード0/-7、
タスクスタック各108byte、静的RAM592byte、起動/カーネルスタック予約512byte。
Flashはtext1797byte＋data初期値20byte（alignmentを除く）。
元の実機ELFとRenvo用LMA補完ELFを分離している。

全待機・wake・BLOCKED/EXITED非選択は同じkernelを使うホスト試験で確認する。
Phase 1のゲストではIRQ禁止を維持し、Idleはカーネルスタック上の有限走査の反復。
BLOCKEDを発生させる公開待機APIとIRQからのwakeはPhase 3以降に統合する。
停止時のebreakはRenvo向け期待停止であり、実機の電源停止ではない。
スタック破損診断とカーネルスタックの最大使用量測定はPhase 4で追加する。
