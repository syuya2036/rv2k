# TCP シェルの入力・表示と RX 状態の削減

このページは PR #18 時点の履歴です。その後、シェル＋ENC は元の容量制約に合格しました。[現在の容量と検査](shell-capacity.md) を参照してください。

シェル＋ENC の Flash 超過を 1,000 byte から 444 byte へ減らしました。まだ元の 16KB linker gate に合格しておらず、Phase 10 は未完了です。仮想 NIC 版は Flash 15,204 / 16,384 byte、全スタック込み RAM 1,908 / 2,048 byte で、通常・IRQ 注入の各 5 接続・12 コマンドが PASS です。

|構成|変更前 Flash / RAM|今回 Flash / RAM|
|---|---:|---:|
|shell 仮想 NIC|15,776 / 1,964|15,204 / 1,908|
|shell＋ENC|1,000 byte 超過 / 有効 ELF なし|444 byte 超過 / 有効 ELF なし|
|HTTP 仮想 NIC|14,984 / 2,020|14,940 / 2,020|
|HTTP＋ENC|16,382 / 2,012|16,322 / 2,004|
|snapshot probe|14,388 / 1,932|14,328 / 1,932|
|TCP＋preempt＋ENC|14,456 / 1,844|14,380 / 1,836|

入力上限は改行を除く 64 byte のままです。保持状態を 16 byte 以下にし、既知名の ASCII byte を u64 に符号化して完全一致で判定します。認識する名前は最大 7 byte、長い語は非ゼロ ASCII を 8 byte 保持するため、末尾の既知名を誤認しません。内部の SP / HTAB を語の結合として扱わず、前後の空白、CR / LF / CRLF、64 / 65 byte、異常入力後の改行復帰と reset を維持します。

buffered reference と 10,108 入力 × 3 分割幅を比較しました。既知名への全 byte 挿入（改行以外）、長い prefix、前後の空白を含み、ハッシュ衝突や短い prefix による近似判定を使いません。最初の実験では解析用定数が connection の frame に退避され、IRQ 時に InvalidSp を検出しました。Line::push を独立した呼出しにして退避量を抑え、最終試験で全番兵を確認しました。失敗時の raw 記録も履歴として残しています。

monitor は NUL 区切りの私有パターン表で task / memory / network の行を選びます。状態名、u8 / u16、符号付き終了コード、RAM 合計も共通処理で出力し、公開 snapshot API と表示は維持します。u64 全域・1,000 個の値、全状態名、i16 の両端、最大 snapshot 値を標準 formatter と照合しました。

ENC は現在の RX token を generation に既に保持しているため、重複した Option<RxFrame> を有効長へ置き換えました。最小長は 60 byte、0 は所有 frame なしです。有効性・長さ・u32 token を別々に検査し、0 長の偽造、異なる長さや token、token wrap、二重 release と stale handle を拒否します。相手先照合・snapshot・neighbor 期限検査では借用を使い、不要な値コピーを避けます。

固定 nightly と最適化フラグ、TCP buffer / MSS、再送・quiet / TIME_WAIT、HTTP の絶対期限、256 byte スタックと全番兵、16KB / 2KB linker 制約は維持します。workspace 88 tests / clippy / fmt、HTTP / shell / snapshot / TCP / compact、強制切替 47 回、SPI bit 3 の IRQ、復元欠陥 3 例、ドライバのモデル・MMIO 試験が PASS です。

シェルの T0 / T1 / kernel 高水位は通常 176 / 236 / 120 byte、IRQ 注入 176 / 208 / 120 byte、全番兵は正常でした。T1 は 240 byte の可用範囲に対して余裕 4 byte なので、今後も IRQ を含む検査を必須にします。実機 ENC の SPI タイミング・スタック実測は別の未検証条件です。

[source / raw log / ELF 検査 / 比較結果](logs/shell-compact)、[shell の raw PCAP / SRAM](logs/shell)、[HTTP と ENC の容量検査](logs/http) に保存しています。比較 JSON はそれぞれ採用前の source を基準に測定した候補で、基準の超過量は regression.json に記録しています。null の超過量は未対応 flag の拒否であり、成功ではありません。以前のフェーズのログは履歴として保持しました。

残るのはシェル＋ENC の 444 byte 削減、Mac native 接続、実機検証です。vmnet 起動は許可済みですが sudo 認証で起動前に停止し、実機も未保有です。仮想 packet peer やリンク成功をこれらの PASS として扱いません。
