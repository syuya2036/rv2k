# Phase 10: TCP シェル統合（実装中）

入力・出力整形のホスト試験は PASS です。ゲスト版は Flash 超過でリンクできておらず、TCP 接続、コマンド実行結果、再接続、スタック高水位の検証は未完了です。以下の動作は実装中のコードの仕様であり、ゲストでの合格結果ではありません。

## 実装

`--features shell` で T0=network、T1=shell の固定 2 タスクと単一 TCP 接続を使います。port 23 の plain TCP で、Telnet 制御は扱いません。banner は `rv2k shell`、prompt は `> ` です。

|コマンド|公開 API|出力|
|---|---|---|
|ps|task::info|2 タスクの PID、状態、終了コード|
|mem|mem::info|RAM / Flash、静的・スタック予約、使用高水位、番兵異常|
|uptime|time::uptime|u64 の tick と tick_ms=10|
|netstat|net::info|TCP 状態、flags、local / peer、フレーム数・エラー・再送・reset|
|help|—|コマンド一覧|
|quit|—|bye を送って接続終了へ進む|

アプリの実行部は公開 API のみを使います。カーネルへの application 領域の登録は別の firmware adapter が行います。行入力と出力はそれぞれ固定配列を持ち、ヒープも第 3 スタックもありません。

入力は CR / LF / CRLF、最大 64 byte です。超過・不正入力は次の改行まで破棄し、エラーを 1 回返して次の行へ復帰します。受信は 1 byte ずつ取り出し、コマンドの出力中にも未処理の後続入力は kernel の TCP RX buffer に保持します。入力用と出力用の同一領域を上書きして複数行を失う構成にはしていません。未完了行は実行せず、新しい接続で入力状態を reset します。

出力は最大 64 byte の行へ分け、部分送信と Again を処理します。1 コマンドの送信は 500 tick の絶対期限で、NET_TX_READY / NET_CLOSED を待って再試行します。対話入力は NET_RX / NET_CLOSED を期限なしで待ちます。

quit 後は未処理入力を実行せず、最大 100 tick 排出して peer の FIN を待ちます。peer が先に閉じなければ active close となり、既存の TCP TIME_WAIT が単一スロットを占有します。再接続を早めるために TIME_WAIT を短縮する変更は入れていません。

## 検証状況

`rv2k-apps` の入力・整形試験は成功しています。整形は最大 snapshot、全状態名、i16 終了コード、IPv4、u64 全域の境界値と 1,000 個の数値を標準 formatter の出力と照合しています。workspace 78 tests、clippy、fmt が PASS です。

固定 nightly / 元の memory.x / 256 byte kernel stack / 既存のサイズ最適化フラグで `shell,emulator-stop` をビルドすると、Flash 16,384 byte の制約を 964 byte 超過します。容量制約は変更していません。リンク失敗のため、ゲスト実行・packet peer・独立 PCAP checker の合格証跡はまだありません。

必要な残作業は、仮想 NIC / ENC 構成の容量収容、guest TCP からの全コマンド実行、分割・連続・超過入力、backpressure、未完了 EOF / RST、切断後の再接続、全番兵検査、CI、統合 PR のレビュー / マージです。Mac native TCP と実機は UNVERIFIED です。
