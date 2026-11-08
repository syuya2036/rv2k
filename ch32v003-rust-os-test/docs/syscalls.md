# Phase 2: RV32E syscall基盤

`rv2k-api`に番号0〜16、負のErrorコード、`task::id/yield_now/info/exit`を追加。
ユーザーwrapperは`a5=番号, a0..a2=引数, a0=結果`でecallする。
a6/a7は使わない。初版の実装対象はYield(0)、GetPid(2)、TaskExit(11)、TaskInfo(12)。
その他の既知番号は後続Phaseの予約でNoSys、未知番号もNoSysを返す。
全番号の試験はサービス全体の実装完了を意味しない。

後続で追加した network snapshot（14）の固定レイアウトと現在の利用可能な snapshot 一覧は [Phase 10 検証](snapshot.md) を参照。以下は Phase 2 当時の実装範囲・測定結果です。

## トラップ境界

`rv2k-arch`は80byteのrepr(C)フレームとRV32E専用asmを持つ。
ra/gp/tp/t0..t2/s0..s1/a0..a5/元sp、mepc/mstatus/mcause/mtvalを保存する。
入口でmscratchとspを交換し、カーネルの一時フレームに全レジスタを退避。
元spの4byte alignment、タスク固有の上下限、16byteのguard、80byte分の空きを検査してからタスクへコピーする。
その後カーネルスタックを巻き戻してdispatchするため、一時フレームをdispatchのcall chainに加算しない。

ecallはmepcを4進め、正常呼出しは同じフレーム、yieldはREADY走査で選んだフレームへmretする。
a0以外のレジスタを復元し、割り込みはPhase 2では禁止のまま。
mscratch=0はカーネル実行中を表し、そこで同期例外が起きると元のカーネルspを復元してfatal停止する。
タスクの壊れたspへstoreしてから検査する経路はない。

TaskInfoは固定6byte・version=1。ポインタの加算overflowとalignmentを検査する。
書込み先は呼出しタスクの保存フレームより上のlive stackに限定し、kernel・他タスク・Flash・guardへの書込みを拒否。
共通のMemoryMapは専用application領域とFlash読み取りも定義するが、このゲストで使う出力はtask stackのみ。
ポインタを復帰後まで保持しない。TCBは公開しない。
M-mode単一アドレス空間なので、これは誤用検出の境界でありメモリ保護ではない。

## 再現と結果

ルートで`cargo test --workspace --locked`、`cargo clippy --workspace --all-targets -- -D warnings`。
固定nightlyとRenvoをセットアップ後:

```sh
cd ch32v003-rust-os-test
bash scripts/syscall.sh
python3 scripts/syscall-negative.py
```

正常系の実測は [syscall-summary.json](logs/syscall-summary.json)。
25279命令、70トラップ、全番号と未知17の検査、カウンタ8/3、タスクreturnとexit(-7)、
同一フレームGetPidと切替Yieldでのレジスタ/スタックsentinelを確認。
不正ID・不正サイズ・misalignment・overflow・他タスク・kernelへの出力を拒否した。
タスクスタック各236/256byte、カーネル100/256byte、静的RAM592byte（タスクスタックを含む）。
Flashはtext3833byte＋data初期値20byte（alignmentを除く）。
タスクの余裕は20byteであり、後続サービス追加時は再測定が必要。

[負例結果](logs/syscall-negative-summary.json)はsp=0の拒否、kernel中ecallのfatal停止、
t1復元命令を抜いたELFでのsentinel検出を記録する。
sp=0検出後にタスク領域へstoreがないこともbus traceで検査する。
大きなbus/coverageログは再現時に生成し、Gitにはsummary・実行結果・サイズ・逆アセンブルを保存する。
CIでもRV32ECビルド、正常/負例、Phase 0/1回帰を実行する。

実機のQingKe CSR/PFIC/HPE設定と電気的な検証は未実施。
Renvo固定版はillegal instructionをゲストへvectorせずCPU faultとして停止するため、
実命令illegal trapの診断はこの試験で検証済みとしない（Phase 4の残ゲート）。

ABIの根拠: [ILP32E psABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_ilp32e_calling_convention)、
[RISC-V privileged architecture](https://docs.riscv.org/reference/isa/priv/machine.html)。
