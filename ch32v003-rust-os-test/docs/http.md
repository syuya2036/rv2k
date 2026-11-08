# Phase 9: HTTP Echo 統合（実装中）

仮想 NIC での HTTP / socket / preemption 試験は PASS です。ENC28J60 構成は Flash 制約に未達で、Mac native HTTP と実機は UNVERIFIED です。Phase 9 全体は完了していません。

## 構成とプロファイル

`--features http,emulator-stop` は T0=network、T1=HTTP の固定 2 タスク、単一 TCP 接続です。HTTP は公開 socket / event / uptime API を使い、MCU 内でリクエスト解析と応答を行います。ホストは対向クライアントとしてパケットを注入・観測し、ゲスト側の TCP / HTTP 処理を肩代わりしません。

`POST /echo HTTP/1.1`、本文 0〜64 byte、Content-Length / Host 必須の制限付きプロファイルです。[パーサ契約](../../crates/rv2k-apps/README.md) に、ヘッダ・Host・エラー応答・非対応機能を記載しています。行と本文で 64 byte 配列を再利用し、socket I/O 用の 128 byte 領域を別に持ちます。ヒープはありません。

公開 `event::wait_until` で期限の検査と登録をカーネル内で一度に行います。残り時間の取得と相対 wait の間の preemption で期限が延びる競合を防ぎます。

受信期限は接続後 500 tick（5 秒）の絶対期限で、途中でデータが届いても延長しません。応答送信も別の 500 tick で打ち切ります。送信 API の部分成功 / Again を扱い、NET_RX / NET_TX_READY / NET_CLOSED を待って再試行します。

Content-Length と Connection: close を出力し、1 接続につき 1 リクエストで終了します。応答を読んだクライアントの FIN と未読要求データの排出を最大 100 tick（1 秒）待ってから close します。クライアントが閉じなければ active close となり、既存 TCP の 120 秒 TIME_WAIT が単一スロットを占有します。この制約を短縮して再接続試験を通してはいません。通常の close-first クライアントは LAST_ACK 完了後に再接続できます。Mac クライアントでの確認は残っています。

## 再現

```sh
cd ch32v003-rust-os-test
bash scripts/http.sh --virtual-only # 仮想 NIC の CI と同じ検査
bash scripts/http.sh                # ENC の strict link まで含む完了ゲート
```

スクリプトは通常試験、RX ごとの IRQ 注入試験、元の ELF の容量検査を実行し、最後に ENC28J60 構成をビルドします。**現時点では最後の ENC リンクが Flash 超過で失敗し、スクリプト全体は非ゼロ終了します。** リンカの 16,384 byte 制約を緩めていません。CI は明示的な `--virtual-only` で仮想 NIC の試験を実行します。引数なしの完了ゲートは引き続き ENC の strict link を必須とし、未達の現在は失敗します。

HTTP の対向 harness はホスト監視時間を 180 秒とします。17 接続・全 230 RX の保護区間 IRQ 注入を低速な CI ホストでも実行するための上限で、ゲストの 200M 命令上限、500 tick の HTTP 絶対期限とその独立検査は維持します。停止時は命令数上限とホスト時間超過を区別し、実行命令数・経過秒を報告します。

結果は [logs/http](logs/http) にあります。`check-http.py` は PCAP の IP/TCP checksum、ストリームの再構築、Python 標準ライブラリの HTTPResponse による応答解析、独立した期待本文、ゲスト SRAM の状態・スタック番兵を照合します。ホスト peer と rv2k パーサの実装を共有しません。PCAP / SRAM の raw log を保存しています。

## 仮想 NIC で確認した内容

通常と IRQ ストレスのそれぞれで 17 接続を実行しました。

- 分割されたヘッダ / 本文、空本文、NUL / 非 ASCII を含む最大 64 byte のバイナリ本文。
- 本文長超過 413、長さ不足 411、重複 Content-Length / Transfer-Encoding / obs-fold / Host 不足 400、Expect 417、ヘッダ行超過 431。
- 途中 EOF 400、部分要求 / 空要求 / 遅い追加データの絶対期限 408、要求途中の RST、その後の再接続。
- SYN-ACK / data ACK / FIN ACK を各 1 回失わせた回復、peer MSS=24、zero-window persist、シーケンス番号 wrap。
- 230 受信フレームを全て解放。応答は 16 回、合計 echo 本文は 88 byte。IRQ ストレスでは NIC RX 中の共有状態の借用を観測して 230 回の tick を注入し、借用解除後に強制切替できることを検査。

|項目|通常|IRQ ストレス|
|---|---:|---:|
|実行命令数|88,090,979|88,342,400|
|RX / TX フレーム|230 / 232|230 / 231|
|強制切替 / 延期 IRQ|15 / 6|4 / 246|
|T0 / T1 / kernel スタック使用 byte|176 / 184 / 120|176 / 184 / 120|
|番兵破損 / fault|0 / 0|0 / 0|

仮想 NIC ELF は Flash 15,478 / 16,384 byte、静的 RAM 1,764 byte + kernel/IRQ/boot 予約 256 byte = 2,020 / 2,048 byte です。タスクの各 256 byte スタックと番兵は静的 RAM に含みます。

ENC28J60 構成は最終リンクで Flash を 460 byte 超過しました。実機用の合格 ELF は生成できていません。`enc-link-failure.txt` が失敗証跡です。

## 共通部分の回帰と未完了ゲート

HTTP と shell は選択式 feature とし、併用や HTTP と snapshot 専用 probe の併用をビルド時に拒否します。公開 snapshot の Service と専有 SPI デバイスを維持して現在の main に統合しました。Content-Length の十進変換は既存の word 除算へ共有し、別の compiler runtime の取り込みを避けて 196 byte 削減しました。

TCB を RV32 で 32 byte stride とし、初期化の一時コピー、固定 MMIO base の保持、syscall wrapper の中間 Result コピーを減らしました。UART/NIC は IRQ マスク中に一度初期化してから使い、デバイスの通常 RAM 状態に volatile な構造体コピーを使いません。OS サービスや TCP 機能は削除していません。

[regression.json](logs/http/regression.json) は初期統合時の共通部分・ホスト NIC 所有権試験の履歴です。現在の統合後は workspace 86 tests / clippy / fmt と、shell / snapshot / TCP の通常・IRQ 注入を再検査して PASS です。[最新回帰](logs/http/integration-regression) と [source / ELF hash](logs/http/verification.json) を保存しています。preemption の除外時間計測は別の `preempt-timing` profile の機能であり、HTTP の `max_disabled_clocks` は未計測（null）です。従来フェーズのログは履歴として保持しています。

未完了は ENC 構成の Flash 収容、Mac native HTTP、Phase 9 統合変更の PR レビュー / マージです。Mac vmnet は実行許可済みですが、前回の `sudo -n` が OS 認証を要求して起動前に失敗しており、ネットワークは作成されていません。[実接続試行](logs/network-live-attempt.json) を参照してください。CH32V003J4M6＋ENC28J60 は未保有で、実機検証は行っていません。

最適化構成は固定 nightly-2026-03-12 の `-Z inline-mir=yes` を使います。全 TCP / snapshot / preemption の最適化試験も同じ設定で再検査します。MIR 設定比較で見つかった相対待機登録の競合は、絶対期限を渡す API で修正します。

[絶対期限の回帰](logs/deadlines) に、残り時間取得後の preemption の反例、両 dispatcher・u32 wrap・期限切れ時のイベント非消費、MIR 構成の preemption / SPI / 診断負例を保存しています。実行予算を超えた harness も PCAP / RAM / 停止理由を保存し、成功としては扱いません。
