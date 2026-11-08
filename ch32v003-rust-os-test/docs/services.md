# Phase 3: タイマー・イベント・IPC

`rv2k-kernel::services::dispatch`がPhase 2のtask syscallに加え、Sleep、Uptime、
EventWait/Signal、MessageSend/Recvを処理する。アプリは`rv2k-api`だけを参照する。
Phase 2のtask専用dispatchとゲストは独立したABI回帰試験として残す。

TCBに待機種別・期限・mask・pending bits・完了結果を固定配置する。RV32でTCBは32byte以下。
期限はu32のwrap差分で比較し、uptimeはu64。有限timeoutは2^31未満、FOREVERはイベントの無期限待機。
sleep(0)はyield、未成立のevent waitでtimeout=0ならTimeoutを返す。
any条件で返したsticky bitだけを消費し、既に成立したイベントをtimeoutより先に処理する。
BLOCKED設定時にも再評価して、通知直前/直後の取りこぼしを防ぐ。
資源に依存するイベント待機は相手タスク終了時にClosedで復帰する。

メッセージはOS全体で64byte・1スロット。満杯はAgain、容量不足はMessageSize。
コピーが成功するまでスロットを公開/消費しないため、不正ポインタや小さい受信領域でメッセージを失わない。
空メッセージも扱う。受信後に送信元へMSG_SPACEを通知し、受信側終了時はスロットを解放する。
通知はヒントなので、アプリはrecv/sendのAgain後に待機・再試行する。
EventWaitはmailboxの実際のready状態も確認する。tickはT0へDRIVER_POLLを通知する。

## SysTickと非再入

SysTickをHCLKの32bit free-running counterとして読み、240000 clockを1 tickとする。
24MHzなら10msが目標だが、実時間の校正は実機ゲートとして残る。
IRQ 12はフラグを解除し次の比較値を設定するだけで、TCBやmailboxへ再入しない。
経過tickはカウンタ差分と端数から求めるので、IRQがマスク中に合流しても時刻を失わない。
カウンタを少なくとも1 wrap以内にsampleする必要がある（24MHzなら約179秒以内）。

タスク中IRQは80byteのカーネル一時フレームから元のPCへ戻る。PCは進めず、まだ強制切替しない。
ecall/idle poll中はMIE=0、共有状態更新を非再入にする。
全待機時はasmでカーネルスタックを巻き戻し、有限pollの間だけIRQを許可する。
その窓にはRust call stackが残っていないので、IRQ用フレームと重ならない。
第3タスク/第3スタックやWFIは追加しない。
待機完了結果は選択時に保存a0へ一度だけ反映し、カーネルのRust call stackを残して切り替えない。
`rv2k_arch::critical`はネスト前のMIEを復元する。区間内のyield/待機は契約上禁止する。

## 再現

```sh
# リポジトリルート
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
# 既存の固定nightly・Renvoセットアップ後
cd ch32v003-rust-os-test
bash scripts/services.sh
```

[実測結果](logs/services-summary.json): 1689649命令、29 syscall、6 timer IRQ、
uptime 7 tick。タスクスタック各104/256byte、カーネル148/256byte、静的RAM736byte
（2タスクスタック、TCB、64byte mailbox、clock等を含む）。総予約RAMは992byte。
Flash text6469byte＋data初期値144byte（alignmentを除く）。
逆アセンブル・ELF配置・サイズ・実行結果も`logs/services-*`へ保存する。
大きなbus/coverageは再現時に生成し、Gitへは追加しない。

ホスト試験はsleep/tick wrap、0/無期限/不正timeout、通知の前後3通り、同tickイベント優先、
遅れたイベントの保持、64byte IPC、満杯・容量不足・不正pointer後の再試行、相手終了を確認。
RV32ECではsleep(0)、タイムアウト、両タスク待機から復帰、満杯からの再開、
5byte/空/4byteのIPC、古い通知後のrecv再試行、公開uptime、ネストしたcritical区間を確認。
2 tickのIRQ禁止中にIRQ回数が増えず、復帰後のuptimeが2 tick以上進むことも検査する。

Phase 0/1/2の回帰結果は [phase3-regression.json](logs/phase3-regression.json)。
以前のphase別ログは当時のcommitでの記録として維持し、現在のkernel変更後のサイズ等は回帰結果へ分ける。
CIでもservicesに加えて既存正常/負例を実行する。
実機の割り込み精度、最大割り込み禁止時間、illegal trap診断、強制切替は後続Phaseで検証する。
