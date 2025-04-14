# 計画: Raspberry Pi USB CDC + ESP-NOW 受信機

## 目的

現状の課題 (中継器でのデータ混信) を解決し、要件 (低消費電力、安定した画像転送) を満たすための最適構成を実装する。

## 概要

1.  **ESP-NOW 受信機 (XIAO ESP32C3)** を Raspberry Pi に USB 接続し、ESP-NOW 受信専用とする。
2.  **カメラ (Unit Cam ESP32-E)** は ESP-NOW で画像データを送信 (変更なし)。
3.  **Raspberry Pi 4B** は USB CDC 経由で画像データを受信、解析、保存する。

## システム構成図 (Mermaid)

```mermaid
graph LR
    subgraph 屋外
        UC1: UnitCam1 -- RX
        UC2[Unit Cam 2 ESP32-WROOM-32E] -- ESP-NOW --> RX
        UC3[Unit Cam N ESP32-WROOM-32E] -- ESP-NOW --> RX
    end
    subgraph 屋内
        RX(XIAO ESP32C3<br/>ESP-NOW受信専用) -- USB CDC --> RPi[Raspberry Pi 4B<br/>データ処理・保存<br/>Python Script<br/>receive_image.py]
    end
```

## 各デバイスの役割と通信 (詳細)

1.  **カメラ (Unit Cam ESP32-WROOM-32E):**
    *   **役割:** 画像取得、ESP-NOW 送信 (変更なし)。
    *   **動作:** 10分ごとに Deep Sleep から起床 → カメラ初期化 → 画像取得 → Wi-Fi 初期化 (STAモード) → ESP-NOW 初期化 → **屋内受信機 (RX) の MAC アドレス** をピアとして登録 → 画像データを ESP-NOW で送信 → Deep Sleep へ移行。
    *   **プロトコル:** ESP-NOW。
    *   **データ:** 画像データ (JPEG 形式など)。送信時に MAC アドレスを付与する必要はありません (ESP-NOW ヘッダに含まれるため)。

2.  **ESP-NOW 受信機 (XIAO ESP32C3):**
    *   **役割:** 複数カメラからの ESP-NOW データを受信し、**USB CDC 経由で Raspberry Pi へ転送**する。
    *   **設置:** Raspberry Pi の USB ポートに接続。電源は Raspberry Pi から供給。
    *   **動作:**
        *   USB CDC ACM デバイスとして初期化 (`examples/usb_cdc` のサンプルコードを参考に実装)。
        *   ESP-NOW を初期化し、受信待機状態にする。
        *   ESP-NOW 受信コールバック関数 (`esp_now_recv_cb`) を実装。
            *   データ受信時にコールバック関数が実行される。
            *   受信データ (ESP-NOW ペイロード) と送信元 MAC アドレス (ESP-NOW ヘッダから取得) を取得。
            *   取得した MAC アドレス (6バイト) と画像データ本体を結合し、USB CDC シリアルポートへ書き込む (データ形式: `[MACアドレス 6バイト][画像データ本体]`)。
    *   **プロトコル (対カメラ):** ESP-NOW。
    *   **プロトコル (対 RPi):** USB CDC (仮想シリアルポート)。

3.  **Raspberry Pi 4B:**
    *   **役割:** USB CDC 経由でデータを受信、解析、保存。
    *   **動作:**
        *   Python スクリプト (`receive_image.py`) を修正。
        *   `pyserial` ライブラリを使用して USB CDC ポート (`/dev/ttyACM0` など) をオープン。
        *   ループで USB CDC ポートを監視し、データを受信する。
        *   受信データから送信元 MAC アドレス (先頭 6バイト) と画像データ本体 (それ以降) を分離。
        *   MAC アドレスごとにデータを区別し、InfluxDB やファイルシステムに保存する処理を実装。
    *   **プロトコル (対受信機):** USB CDC (仮想シリアルポート)。

## タスクリスト

1.  **ESP-NOW 受信機 (XIAO ESP32C3) ファームウェア開発 (`examples/usb_cdc` を参考に `examples/usb_cdc_receiver` プロジェクトを新規作成):**
    *   プロジェクト作成 (`examples/usb_cdc_receiver`): `examples/usb_cdc` をコピーして `examples/usb_cdc_receiver` を作成し、不要なファイルを削除・整理する。
    *   `Cargo.toml` 修正: プロジェクト名、依存クレートなどを修正。
    *   `src/main.rs` 修正:
        *   USB CDC 初期化処理を実装 (`examples/usb_cdc` を参考にする)。
        *   ESP-NOW 受信処理を実装 (`examples/image_receiver` を参考にする)。
        *   ESP-NOW 受信コールバック関数 (`esp_now_recv_cb`) を実装。
            *   受信データと送信元 MAC アドレスを取得。
            *   データ形式 `[MACアドレス 6バイト][画像データ本体]` で USB CDC ポートへ書き込む。
    *   ビルドと動作確認: XIAO ESP32C3 にファームウェアを書き込み、USB CDC 経由でデータ送信できることを確認する (テスト用 ESP-NOW Sender を別途用意するか、`image_sender` を流用)。

2.  **Raspberry Pi `python_server` (`examples/python_server/receive_image.py`) の改修:**
    *   `receive_image.py` を修正:
        *   `pyserial` ライブラリをインストール (`pip install pyserial`)。
        *   USB CDC ポート (`/dev/ttyACM0` など) を指定可能にする (引数または設定ファイル)。
        *   `pyserial` を使用して USB CDC ポートをオープンし、データ受信処理を実装。
        *   受信データから MAC アドレスと画像データを分離する処理を実装 (データ形式 `[MACアドレス 6バイト][画像データ本体]` を前提)。
        *   MAC アドレスごとに画像ファイルを保存する処理を実装 (ファイル名に MAC アドレスを含めるなど)。
    *   動作確認: `python_server` を実行し、ESP-NOW 受信機 (XIAO ESP32C3) から送信されたデータを受信・保存できることを確認する。

3.  **カメラ (Unit Cam ESP32-E) ファームウェア確認 (`examples/image_sender`):**
    *   `examples/image_sender` のコードを確認し、ESP-NOW 送信部分が最適化案の構成 (受信機の MAC アドレスをピアとして登録、画像データを送信) に合致しているか確認する。必要であれば修正する。

4.  **システム結合テスト:**
    *   カメラ (Unit Cam ESP32-E) を複数台用意し、同時にデータ送信を行う。
    *   ESP-NOW 受信機 (XIAO ESP32C3) を Raspberry Pi に接続し、`python_server` を実行する。
    *   複数カメラからの画像データが Raspberry Pi で正しく受信・保存されることを確認する。
    *   データ欠落やエラーが発生しないか、安定性を確認する。

## 実装のポイント

*   **ESP-NOW 受信機 (C3) ファームウェア:**
    *   USB CDC ACM デバイスとしての初期化を確実に行う (`examples/usb_cdc` を参考に)。
    *   ESP-NOW 受信コールバック関数 (`esp_now_recv_cb`) で、受信データと MAC アドレスを正しく取得する。
    *   データ形式 `[MACアドレス 6バイト][画像データ本体]` を厳守し、USB CDC ポートへ書き込む。
    *   エラー処理 (ESP-NOW 受信エラー、USB CDC 書き込みエラーなど) を適切に実装する (必要に応じて)。

*   **Raspberry Pi (Python スクリプト):**
    *   `pyserial` で USB CDC ポートを正しくオープンし、データ受信処理を実装する。
    *   受信データの形式 (`[MACアドレス 6バイト][画像データ本体]`) に基づいて、MAC アドレスと画像データを正しく分離する。
    *   MAC アドレスごとの画像保存処理を実装する。
    *   エラー処理 (USB CDC 受信エラー、ファイル保存エラーなど) を適切に実装する (必要に応じて)。

## テストと検証項目

*   **単体テスト:**
    *   ESP-NOW 受信機 (C3) ファームウェア: ESP-NOW 受信、USB CDC 送信の動作確認。
    *   `python_server`: USB CDC 受信、データ分離、ファイル保存の動作確認。
    *   カメラ (ESP32-E) ファームウェア: ESP-NOW 送信の動作確認 (既存の `image_sender` で確認可能)。

*   **結合テスト:**
    *   システム全体のデータフロー確認 (カメラ -> 受信機 -> Raspberry Pi)。
    *   複数カメラからの同時データ受信時の動作確認。
    *   データ欠落、データ破損の有無の確認。
    *   エラー発生時の挙動確認 (エラーログ出力、リトライ処理など)。

*   **性能評価:**
    *   データ転送速度の測定 (カメラ -> Raspberry Pi)。
    *   システムの安定性評価 (長時間連続運転テスト)。
    *   CPU 使用率、メモリ使用量などのリソース使用状況のモニタリング (必要に応じて)。

## 実装結果と課題解決

### 1. ESP-NOW + USB CDC 方式の実装状況

**実装完了項目:**
* XIAO ESP32C3を用いたUSB CDC受信機の実装 (examples/usb_cdc_receiver)
* ESP-NOW受信、フレーム作成、USB CDC転送機能の実装
* PythonスクリプトによるUSB CDC受信処理の実装
* カメラ〜ラズパイ間のEnd-to-Endデータ転送の確認

**遭遇した主な課題と解決策:**

1. **USB CDC転送のタイムアウト問題:**
   * **課題**: ESP-NOWで受信したデータをUSB CDC経由でRaspberry Piに転送する際、データが部分的に送信されず、タイムアウトエラーが発生した。
   * **解決策**: 
     * USB CDCバッファサイズを2048バイトに増加
     * 送信処理を小さなチャンクサイズ(64バイト)に分割
     * エラーハンドリングとリトライ機能の強化
     * タイムアウト時間の延長(30秒)

2. **データフレーミングとデリミタの問題:**
   * **課題**: 送信データにデバッグログやその他制御文字が混入し、受信側でのデータ解析が困難になった。
   * **解決策**:
     * 明確なフレームマーカー(0xAAAA〜0xBBBB)を使用したデータフレーミングの実装
     * フレーム内にMACアドレスとデータ長の情報を含め、受信側での解析を容易に
     * EOFマーカー(b"EOF!")の導入によるデータ転送完了の明示化

3. **FreeRtos型のインポート問題:**
   * **課題**: FreeRtos型が未定義であるコンパイルエラーが発生した。
   * **解決策**: esp_idf_svc::hal::delay::FreeRtosを正しくインポート

### 2. 動作確認結果

* **基本機能**: カメラモジュールから撮影した画像(約16.7KBのJPEG)がRaspberry Piで正常に受信・保存できることを確認
* **画像転送**: 1分ごとの画像転送が正常に動作
* **画像保存**: 受信した画像がJPEG形式でRaspberry Pi上のファイルシステムに正常に保存

### 3. 残課題

1. **データ転送の最適化:**
   * USB CDC通信中にログメッセージが混入する問題の解消
   * "Discarding X bytes before start marker"警告の削減
   * より効率的なフレーム処理方法の検討

2. **複数カメラのテスト:**
   * 複数台のカメラからの同時データ受信テストがまだ未実施
   * 同時受信時のデータ混信や欠落がないかの検証

3. **長期安定性:**
   * 長時間運用時のシステム安定性の検証
   * メモリリーク等の問題がないかの確認

4. **その他改善点:**
   * ログレベルの調整によるノイズ削減
   * エラー時のリカバリー処理の強化
   * 監視・診断機能の強化