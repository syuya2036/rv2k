# HTTP＋ENC28J60 の Flash 収容

HTTP 構成が元の `memory.x` で Flash 16,382 / 16,384 byte、全スタック込み RAM 2,012 / 2,048 byte に収まりました。HTTP の通常・IRQ 注入試験と最後の ENC strict link を含む `bash scripts/http.sh` 全体が PASS です。CI もこの引数なしの完全な検査を使います。実機と Mac native HTTP は未検証です。

固定 nightly-2026-03-12 の MIR inlining を維持し、LLVM のインライン展開しきい値を 25 としました。TCP の終了処理、ソケットのハンドル検証・readiness・terminal、ENC の PHY 読出しに `inline(never)` を付け、呼出し箇所ごとのコード複製を抑えます。TCP の状態遷移、MSS / buffer サイズ、再送・quiet / TIME_WAIT、公開 API、診断機能、リンカ上限は維持しています。

|構成|変更前 Flash|変更後 Flash|予約 RAM|
|---|---:|---:|---:|
|HTTP 仮想 NIC|15,478|14,984|2,020|
|HTTP＋ENC|460 byte 超過|16,382|2,012|
|shell 仮想 NIC|16,288|15,776|1,964|
|shell＋ENC|1,484 byte 超過|1,000 byte 超過|有効 ELF なし|
|snapshot probe|14,860|14,388|1,932|
|TCP＋preempt＋ENC|14,892|14,456|1,844|

Flash 余裕は HTTP＋ENC で 2 byte です。今後の増加は strict link と元 ELF の load span 検査で拒否します。実機での SPI タイミングや全呼出し経路のスタック実測は、このリンク結果だけでは証明していません。

通常 / IRQ 注入で HTTP はそれぞれ 17 接続、16 応答、88 byte の本文一致、230 RX の全解放、EOF / RST / 再接続、500 tick 絶対期限と損失回復が PASS です。T0 / T1 / kernel 高水位は両方 176 / 176 / 120 byte、全番兵が正常でした。raw PCAP / SRAM と元 ELF hash は [HTTP ログ](logs/http) に保存しています。

shell は通常 / IRQ 注入で 5 接続ずつ、全 4 ユーティリティ、入力復帰、EOF / RST / 再接続が PASS です。高水位は通常 176 / 232 / 120、IRQ 注入 176 / 216 / 120 byte でした。[shell ログ](logs/shell) を参照してください。ENC 構成の残り 1,000 byte の削減を継続します。

workspace 86 tests、clippy、fmt と、TCP / compact / snapshot の各試験が PASS です。非 yield CPU probe の強制切替 47 回、SPI の bit 3 での IRQ、復元欠陥 3 例、TCP / preempt profile の診断負例各 9 件も検出できました。各タスク・kernel の guard は正常です。既存フェーズのログを履歴として残し、新しい共通回帰は [flash-fit ログ](logs/flash-fit) へ保存しました。

同ディレクトリの比較 JSON は、採用前の各候補を単独に測定した結果です。展開抑制候補の比較は LLVM しきい値 25、前半は旧 source、後半は `finish` / `validate` の抑制を加えた source が基準です。採用構成は `regression.json` と実行スクリプトを正とします。解析用 32KB ELF はサイズ調査だけに使用し、実行・容量合格の根拠に使っていません。

Mac vmnet は実行許可済みですが、`sudo -n` が認証を要求し、ネットワーク作成前に停止しました。実機は未保有です。これらを仮想 packet peer の成功で代替せず、README の Phase 9 / 10 は未完了のままとしています。
