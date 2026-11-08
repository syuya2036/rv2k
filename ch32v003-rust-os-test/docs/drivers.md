# Phase 5: ドライバ基盤（PARTIAL）

UART・GPIO所有権・software SPI・ENC28J60・NetworkDeviceを実装した。
ホストのモデル試験とRV32ECでのMMIO実行はPASS。**J4M6＋ENC28J60実機は未保有のため、
電気的成立性、実SPI通信、速度、SWIOでの再書込みはUNVERIFIED。Phase 5全体はPARTIALのままとする。**
このゲートをモデルで代替せず、後続のプロトコル開発を並行して進める。

## 境界と所有権

`rv2k-drivers`はno_std、ヒープなし。NetworkDeviceはpoll、RXのbegin/read_at/release、
TXのbegin/write/commit/abortを定義する。フレームはNIC内に置き、MCUでは小さいscratchだけを使う。
RX handleは世代付きで、解放後の再読取り・二重解放を拒否する。借用中にyieldしない。
SPIはmode 0・MSB firstで、トランザクション全体のCSを保持する。転送全体のIRQマスクは行わない。
同じSPIを他タスクに使わせず、将来のpreemption延期はPhase 8で統合する。

J4M6のピンはREADMEの配線表に一致する。PA1/PD6、PD1/PD4/PD5を同じ物理所有権へまとめる。
予約済みGPIOはBusy、外部に出ないPC5/6/7はInvalid。GPIO aliasは入力、PD1/SWIOは変更しない。
出力ラッチでCSを先にhigh、SCK/MOSIをlowにしてからGPIO modeを変更する。
UARTはPD6へのremap 2・TXのみ。AFIOのSWIO関連bitは保持し、HSI構成でPA12_RMをclearする。
`board::j4m6()`は24MHz HSIとレジスタの排他的所有を要求するunsafe境界。

UARTは32byteリング。writeはコピーした部分長またはAgainを返す。pollは有限byte数のみ送る。
ユーザー入力をリングへ直接コピーし、失敗したコピーは公開しない。追加の32byte stack scratchは不要。
公開`console::write`はsyscall 6（fd=1）を使う。kernelはBadFd・Faultを区別し、アプリのポインタを保持しない。
NIC待ちやUART故障でOSサービスを無限待機させない。

## ENC28J60

SPIのbank切替、MAC/MII読取りのdummy byte、soft reset後2ms以上の待ち、PHYの有限busy待ちを実装。
slow SPIのMAC erratumがあるB1/B4は拒否し、B5/B7（EREVID=5/6）を対象にする。
半二重、MAC padding/CRC、unicast/broadcastとCRC filterを初期化し、PHYからリンク状態を読む。
RXは0..0x17ff、TXは0x1800以降のNIC内RAM。FCSを除く最大1514byteを部分転送する。
RX countはPKTIFではなくEPKTCNTを読み、length/status/next pointerを検査する。
RX ringの折返しと再読取りを扱い、解放はnext-1（0への折返しならRX_END）のodd値をlow→high順で書く。
不正status/lengthは解放し、壊れたnext pointerやSPI障害はreset必須の状態へ移す。

TXは全byteを書き終えるまでcommitしない。busyはAgain。abort・異常・期限超過でTXRTSをclearし、
TXRSTをtoggleしてTXIF/TXERIFを解除する。期限はOSのu32 tickで100 tick、wrap比較を使う。
TX failureは上位へ通知し、TCP再送のデータ所有権をNICへ移さない。MACの衝突再試行以上の
独自フレーム再送は行わず、ネットワーク層の再試行へ返す。
全SPIエラーでCSのdeassertを試み、状態を破棄してresetするまで利用を拒否する。

## 再現と検証範囲

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cd ch32v003-rust-os-test
bash scripts/setup-emulator.sh
bash scripts/drivers.sh
```

[ホスト試験](logs/drivers-host-tests.log)は計30件（driver 12件）。独立したSPI register/RAM modelで、
bank/dummy/MII/reset/MAC順序、1514byteの分割RX/TX、RX ring wrap・再読取り・二重解放拒否、
不正status/length/next、TX未完成/busy/error/abort/deadline wrap、SPI障害・reset復旧、
未接続/旧revisionの拒否を確認する。pin/alias、SPIエッジ順、UARTの部分write・wrap・失敗コピーも試験する。
モデル用8KB RAMとホストの配列はMCUにはリンクしない。

[RV32EC実測](logs/drivers-summary.json): 1871502命令、31 syscall、6 timer IRQ。
公開writeから`UART via API`を出し、従来のtimer/IPC/memory監視も同時に動く。
GPIO bus traceからSPIの56 rising edgesを復元し、reset/bank切替/EREVID読取りの4トランザクションを検査する。
PA1 alias入力、USART remap 2、PA12 GPIO、SWIO保持も実際のMMIO writeから確認する。
このRenvo構成ではMISO=0、NIC接続なしの拒否が期待結果。**この結果を実NICとの通信成功とは呼ばない。**

[実機用ELFの予算](logs/drivers-budget.json): Flash12992/16384byte（load alignment込み）、
静的RAM848＋kernel stack256＝1104/2048byte。タスクwatermarkは各184/256、kernel220/256byte。
すべてのENC streaming操作はRV32ECリンクに含まれるが、実NICのRX/TX call chainはゲストで未実行。
パケットを受信するPhase 6以降で追加の最大スタック・Flashを計測する。
[Phase 0〜4回帰](logs/phase5-regression.json)もPASS。大きなbus/coverageは再現時に生成する。

## 残る実機ゲート

J4M6＋B5/B7 ENC28J60、共通GNDと適切な電源、CSの外部pull-up、NICの25MHz clock/reset回路、
SWIOの書込みアダプタを用意し、READMEの物理ピン表と導通を確認する。
SWIOを保持した繰返し書込み、MISO実読取りとreset/link、SPI mode/周波数、フレーム転送時間、
最大IRQ禁止時間、ポーリング周期とRX overflowを測る。これらは未実施である。

根拠: [WCH GPIO定義](https://raw.githubusercontent.com/openwch/ch32v003/main/EVT/EXAM/SRC/Peripheral/inc/ch32v00x_gpio.h)、
[ENC28J60 datasheet](https://ww1.microchip.com/downloads/en/DeviceDoc/39662e.pdf)、
[Microchip silicon errata](https://ww1.microchip.com/downloads/en/DeviceDoc/80349c.pdf)。
