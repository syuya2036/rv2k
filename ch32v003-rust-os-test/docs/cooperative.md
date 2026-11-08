# 協調マルチタスク実動検証

2タスク固定・各256バイトの独立スタックで、Rustタスクの協調切り替えをApple Silicon macOS上のRenvoで確認した。割り込みは禁止したまま、ヒープは使わない。実機での検証はまだ行っていない。

## 実行結果

```text
UART: ABABABABABABABAB\nCOOP OK\n
COUNTERS: [8, 8]
SWITCHES: 32
COOP_STATUS: 0x600d
instructions: 6112
```

- タスクA/Bがそれぞれ8回カウンタを更新し、RAMのTRACEもUARTと同じAB交互順。
- 1ラウンドに、アセンブリのレジスタ検証経由と通常のRust関数 `yield_task()` 経由で1回ずつyieldする。A/B合わせて32回のスイッチ。
- `gp`, `tp`, `t0..t2`, `s0..s1`, `a0..a5` にA/Bそれぞれ異なる値を入れ、相手のタスクが同じレジスタを変更した後も全て復元されたことを検証。
- `ra` はyieldの次の命令へ戻ることで確認。`sp` は独立したローカル変数、スタック上のsentinel、保存SP範囲、アラインメントを確認。
- Rustのvolatileローカルカウンタと8ワードのcookieを跨いで保持。ローカルカウンタの実アドレスはA=`0x20000120`, B=`0x20000220`。
- 各スタック下端16バイトのcanaryが保持され、watermarkによる使用量はA/Bとも172バイト。これは今回のビルドと実行経路の観測値で、任意の将来コードの最大スタック量保証ではない。
- `a5` 復元命令をNOPに差し替えた使い捨てELFは `COOP_STATUS=3` で停止し、成功メッセージを出さない。レジスタ保持テストが欠陥を検出することも確認した。

根拠: [実行サマリ](logs/coop-summary.json)、[CPUとUART](logs/coop-run.json)、[SRAMアクセス](logs/coop-bus.json)、[逆アセンブル](logs/coop-disassembly.txt)、[負例ログ](logs/coop-negative.log)。`check-coop.py` はシンボル表から実アドレスを取得し、SRAM書込みをリプレイして最終状態を照合する。

## 実装とABI

[src/coop.rs](../src/coop.rs) に初期化・固定2タスク・コンテキスト切替・UART・RAM状態を実装。[coop_probe.S](../src/coop_probe.S) は検証用レジスタsentinel処理。

RV32Eのx0は不変。x1〜x15の15レジスタを、64バイトのスタックフレームに保存する。割り込みコンテキストではなく通常関数呼出しの協調コンテキストなので、再開PCは `ra` を用い、`mepc/mstatus` の切替はしない。今回の同一特権・割り込み禁止という前提に限定する。

|オフセット|内容|
|---|---|
|0|ra（再開PC）|
|4, 8|gp, tp|
|12, 16, 20|t0, t1, t2|
|24, 28|s0, s1|
|32〜52|a0〜a5|
|56|フレーム作成前のsp|
|60|パディング|

スケジューラが使う一時レジスタも、使用する前に保存する。`SAVED_SP[CURRENT_TASK]` に現在フレームを記録し、`CURRENT_TASK ^= 1`、次の保存SPへ切替、全レジスタを復元して `ret`。独立したスケジューラ用スタックやヒープは不要。

ILP32Eでは通常の関数呼出しでcaller-savedレジスタが変更されること自体は正しい。ここでは切替ルーチンの入口から出口まで全レジスタを保持する強い実装にしているが、Rustの呼出し側がyieldの前後に生成するコードまでcaller-saved値の不変を約束するものではない。そこで、全レジスタ検証は専用アセンブリから直接yieldを呼び、Rust側はABIに従ったlive local保持も別途検証している。

各スタックの開始は16バイト境界（ILP32Eの必要条件は4バイト）。256バイトとフレーム64バイトでアラインメントを維持する。初回はフレームをゼロ初期化し、`ra=task_a/task_b`, `sp=各スタック上端`, `gp/tp=起動元の値` を設定する。初回専用 `first_task` は通常の復元経路に合流する。戻り値を返すタスクは扱わず、入口は `extern "C" fn() -> !`。

Rust呼出しAPIは `yield_task()`。初期化後のタスク文脈だけから呼ぶ。固定の次タスクへ移る方式で、タスク追加、sleep、終了、待機キュー、プリエンプション、割り込みネストにはまだ対応しない。

## メモリ予算

|領域|バイト|
|---|---:|
|タスクAスタック|256|
|タスクBスタック|256|
|状態変数・保存SP・パディング|80|
|静的RAM合計|592|
|既存の起動スタック予約|512|
|RAM予算合計|1104 / 2048|
|未割当の余裕|944|

Flashのtextは1553バイト、dataは0バイト。Flash/RAMの領域制限と、静的領域が起動スタック512バイトへ重ならないリンカASSERTを維持する。タスクスタック512バイトは静的RAM592バイトに含まれており、二重計上しない。

タスクスタックは `TASK_STACKS` の静的実体としてリンカに計上される。起動用の512バイトは最初のタスクへ移った後は使用しないが、既存構成との共通性のため予約を残す。canaryはMMU等の保護機構ではなく、今回の実行後の破壊検出である。

## 再現手順

[README](../README.md) のツールチェーン・Renvoセットアップ完了後:

```bash
cd ch32v003-rust-os-test
./scripts/coop.sh
python3 scripts/coop-negative.py
```

期待する出力:

```text
PASS: counters=8/8, switches=32, register probes passed; stack watermark A/B=[172, 172] bytes; 6112 instructions
PASS: negative test detected missing a5 restore at 0xd6; COOP_STATUS=3
```

負例の命令アドレスは固定ツールチェーンの今回の値。スクリプトは逆アセンブルから対象命令を探す。ソースコードや正常版ELFは変更しない。

`coop.sh` は `target/coop/` に別ビルドする。元の実機向けELFと、従来のLMA変換を通した `.remu` ELFを分離し、既存の通常版・タイマー版の出力を上書きしない。

タスクは無限ループとして実装し、タスク終了APIは設けていない。テストはA/Bが8回更新した後、Aの実行中に `ebreak` でエミュレータを停止する。この期待停止はタスク終了ではなく、再現可能な有限の検証点である。

既存機能の回帰確認も実行した:

```bash
./scripts/build.sh
./scripts/run.sh
./scripts/timer.sh
./scripts/build.sh
./scripts/run.sh
```

通常版174命令、SysTick版304命令の検証が引き続き成功。`coop` と `timer` の同時指定はcompile_errorで拒否する。

## デバッグ用状態とエラー

`CURRENT_TASK`, `SAVED_SP`, `SWITCHES`, `COUNTERS`, `LOCAL_ADDR`, `STACK_USED`, `TRACE`, `TRACE_LEN`, `COOP_STATUS` をシンボルとして公開。成功時 `COOP_STATUS=0x600d`、失敗は下記値を書いて `ebreak` 停止する。

|値|検出内容|
|---|---|
|1|カウンタまたは交互実行順序|
|2|トレース範囲超過|
|3|アセンブリのレジスタ／スタックsentinel不一致|
|4|Rustローカルcookie不一致|
|5|Rustローカルカウンタ不一致|
|6|終了確認点でBのカウンタ不一致|
|7|スタック下端の余裕不足／破壊|

`STACK_USED` はテスト完了点で算出する。`COOP_STATUS=0` はまだ完了していない状態。命令上限到達を成功扱いにしない。
