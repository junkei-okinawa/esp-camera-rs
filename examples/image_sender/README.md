# ESP32 Camera ImageSender

[M5Stack Unit Cam Wi-Fi Camera (OV2640)](https://docs.m5stack.com/en/unit/unit_cam)で撮影した画像をESP-NOWプロトコル経由で送信するソフトウェアです。

## プロジェクト概要

このプロジェクトは、ESP32マイクロコントローラーとカメラモジュールを使用して構築されており、以下の主要な機能を提供します：

- ESP32カメラモジュールによる定期的な画像撮影
- 撮影した画像のSHA256ハッシュ計算と検証
- ESP-NOWプロトコルを使用した画像データのチャンク送信
- 省電力のためのディープスリープ制御
- LED表示によるステータス通知

## 受信機との連携

このプロジェクトは、同じリポジトリ内の`usb_cdc_receiver`プロジェクトと連携して動作します。送信機（このプロジェクト）が撮影した画像を、受信機（`usb_cdc_receiver`）がESP-NOWプロトコルで受信し、USBシリアル経由でPCに転送します。

詳細な受信機の設定と使い方については、[usb_cdc_receiver README](../usb_cdc_receiver/README.md)を参照してください。

## アーキテクチャ

プロジェクトは以下のモジュールで構成されています：

### モジュール構成

- **camera**: カメラ初期化と画像キャプチャ処理
- **config**: 設定ファイルからの構成読み込み
- **esp_now**: ESP-NOWプロトコル通信と画像フレーム処理
- **led**: ステータス表示用LEDの制御
- **mac_address**: MACアドレス処理
- **sleep**: ディープスリープ制御

### データフロー

1. ESP32カメラで画像を撮影
2. 撮影した画像のSHA256ハッシュを計算
3. 画像データをチャンクに分割
4. ESP-NOWプロトコルを使用して受信機に送信
   - 最初にハッシュ情報を送信
   - 次に画像データをチャンクに分けて送信
   - 最後にEOFマーカーを送信
5. 送信完了後、ディープスリープに移行
6. 一定時間後に再び起動して次の撮影サイクルを開始

## 使用方法

### 必要条件

- Rust（1.71以上）
- ESP-IDF（v5.1以上）
- ESP32 + カメラモジュール (本プロジェクトでは`Unit Cam Wi-Fi Camera (OV2640)`を使用)
- cargo tools：`cargo-espflash`

### セットアップと構成

1. リポジトリをクローン：
   ```bash
   git clone https://github.com/junkei-okinawa/esp-camera-rs.git
   cd esp-camera-rs/examples/image_sender
   ```

2. 設定ファイルのセットアップ：
   ```bash
   cp cfg.toml.template cfg.toml
   ```

3. `cfg.toml`を編集して、受信機のMACアドレスとディープスリープ時間を設定：
   ```toml
   [image-sender]
   receiver_mac = "1A:2B:3C:4D:5E:6F"  # 受信機のMACアドレスに変更
   sleep_duration_seconds = 60         # ディープスリープ時間（秒）
   ```
   
   `sleep_duration_seconds`の値を変更することで、撮影と送信の間隔を調整できます。例えば、5分間隔で撮影したい場合は`300`に設定します。

### 異なるカメラモジュールへの対応

異なるESP32ボードやカメラモジュールを使用する場合は、以下の設定が必要になります：

1. `.cargo/config.toml`の修正：ESP32のターゲットに合わせて変更
2. `sdkconfig.defaults`の変更：PSRAMの設定などを調整
3. カメラのピン設定：`src/camera/controller.rs`のピン設定を変更

### ビルドと書き込み

プロジェクトをビルドして、ESP32デバイスにフラッシュするには：

```bash
cargo espflash flash --release --port /dev/your-port --monitor
```

## テスト

### 単体テスト実行

コードの単体テストを実行するには：

```bash
cargo test --lib
```

PCの開発環境でテストを実行する場合は問題ありませんが、ESP32実機上でテストを実行する際には注意が必要です：

```bash
cargo test --lib --target xtensa-esp32-espidf
```

※注：一部のテストはESP32実機環境では実行できないため、`#[ignore]`属性でマークされています。

テストカバレッジ：

- mac_address: MACアドレス処理のテスト
- esp_now/frame: ハッシュ計算やメッセージ準備のテスト
- camera: カメラ制御のテスト（ハードウェア依存）
- led: LEDパターン制御のテスト（ハードウェア依存）

## モジュール解説

### camera

カメラの初期化、設定、画像撮影を担当します。M5Stack Unit Camなどの異なるカメラモジュールに対応できるよう、柔軟なピン設定が可能です。

### config

設定ファイル（cfg.toml）から受信機のMACアドレスなどの構成情報を読み込みます。

### esp_now

ESP-NOWプロトコルを使用した通信処理を担当します。主な機能として：
- 送信機の初期化とピア登録
- 画像データのチャンク分割送信
- ハッシュ計算と検証

### led

ステータスLEDの制御を行います。撮影中、送信中、エラー状態などを異なるLEDパターンで表示します。

### mac_address

MACアドレスの解析、検証、文字列変換などの機能を提供します。

### sleep

ディープスリープの制御と電力管理を担当します。撮影・送信サイクルの間の省電力化に貢献します。

## トラブルシューティング

### カメラが認識されない場合

- カメラのピン設定を確認してください。本ソフトウェアは**[Unit Cam Wi-Fi Camera (OV2640)](https://docs.m5stack.com/en/unit/unit_cam)**で動作するように設定されています。
- 電源供給が十分か確認してください

### 画像送信が失敗する場合

- 受信機が起動しているか確認してください
- MACアドレスが正しく設定されているか確認してください
- 送信機と受信機の距離が遠すぎないか確認してください

### ディープスリープが正常に動作しない場合

- ボードがディープスリープに対応しているか確認してください
