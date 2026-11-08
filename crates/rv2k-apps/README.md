# rv2k application protocols

`no_std`、ヒープなしのアプリケーション用プロトコル処理です。カーネルや NIC には依存しません。

## HTTP Echo プロファイル

`http::Request` は `POST /echo HTTP/1.1` を増分解析します。本文は最大 64 byte、バイナリのまま保持します。行と本文で同じ 64 byte 配列を再利用し、受信チャンク境界に依存しません。

- CRLF、Host 1 個、Content-Length 1 個が必須です。Content-Length は符号なし十進数で、0 も受け付けます。
- 行は CRLF を除いて最大 64 byte、ヘッダは最大 16 個、リクエスト行を含むヘッダ全体は最大 1,024 byte です。
- Host は ASCII の DNS 形式のラベル / IPv4 名と、省略可能な十進ポート（0〜65535）を受け付けます。末尾の root dot は許可し、IPv6 literal、userinfo、percent encoding は扱いません。
- フィールド名は ASCII tchar、値は HTAB / printable ASCII に限定します。重複 Content-Length、Transfer-Encoding、obs-fold、コロン前の空白は拒否します。Expect は 417 で拒否します。
- 非対応の method / path / version は 400、Content-Length 不足は 411、本文長超過は 413、リクエスト行超過は 414、ヘッダ超過は 431 です。期限切れは 408、未完了の EOF は 400 です。
- 1 接続につき 1 リクエストです。完了後の余剰バイトを本文に含めず、エラー時の本文は空です。reset 後も前の本文を公開しません。

`response_header` は Content-Length、Content-Type: application/octet-stream、Connection: close を出力します。reason phrase は共通の `Response`、長さは `00`〜`64` の十進表記です。本文は呼び出し側が別に送信します。

完全な HTTP/1.1 実装ではありません。socket による待機、受信期限、送信の再試行と接続終了は firmware 側の責務です。MCU 上の統合、容量制約、Mac との通信は別の検証項目です。

## 検証

リポジトリルートで実行します。

```sh
cargo +stable test -p rv2k-apps --locked
cargo +stable clippy -p rv2k-apps --all-targets -- -D warnings
```

0〜64 byte の全本文長、バイナリ本文、1 byte ずつの入力、最大本文の全 2 分割点、不正 framing / Host / 制限値、EOF / timeout / reset、および全ステータス × 全本文長の応答ヘッダを検証します。

## TCP シェルの行入力（Phase 10）

`shell::Line` は 64 byte 固定配列を持ち、`push(byte)` が改行ごとに `Command` を返します。`ps` / `mem` / `uptime` / `netstat` / `help` / `quit` を大文字小文字を区別して認識し、前後の SP / HTAB を無視します。引数や空白のある別コマンドは Unknown、空行は Empty です。

CR、LF、CRLF に対応し、分割された CRLF を 1 改行として扱います。制限は前後の空白を含み、改行を除いて 64 byte です。超過または不正な byte の後は、次の改行まで全て捨ててエラーを 1 回返し、その次の行から復帰します。許可する入力は printable ASCII と HTAB で、backspace / terminal escape / 非 ASCII / Telnet 制御は扱いません。

呼び出し側は新しい TCP 接続で `reset()` し、EOF の未完了行は実行しません。socket から複数行が一度に届いた場合も、全 byte を順番に push することで全コマンドを処理できます。このモジュールは行入力とコマンド識別だけを実装しており、公開 snapshot API による状態取得・送信や TCP シェル全体の完了を意味しません。

試験は全チャンク幅 / 全 2 分割点、改行種別、64 / 65 byte 境界、5,000 byte の超過行からの復帰、全 byte 値の分類、接続変更時の reset を含みます。上記の test / clippy コマンドで HTTP と一緒に実行できます。

## OS snapshot の表示

`monitor::Line` は公開 TaskInfo / MemInfo / NetInfo と uptime を、最大 64 byte の行へ整形します。task は 1 行、memory は 3 行、network は 4 行に分けます。IPv4 と整数は十進表記で、uptime は u64 全域、終了コードは i16 全域に対応します。u64 の変換は 16 bit limb ごとの u32 除算で行い、core::fmt やヒープは使いません。

最大値の snapshot、全状態名、負の終了コード、IPv4 の先頭ゼロ除去、u64 の境界値と 1,000 個の値を標準 formatter と照合します。socket 送信、接続中の取得結果、MCU スタック高水位は firmware の統合検証項目です。
