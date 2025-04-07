# 計画: ESP-NOW 画像転送における送信元デバイス識別

## 目的

複数の `image_sender` (ESP32) から `image_receiver` (ESP32) を経由して `python_server` (Raspberry Pi) に送信される画像を、受信側で保存する際に、どの `image_sender` デバイスから送信された画像なのかをファイル名で区別できるようにする。

## 方針

`image_receiver` が ESP-NOW でデータを受信する際に送信元 MAC アドレスを取得できるため、この情報を利用する。`image_receiver` は取得した MAC アドレスを、画像データと共にシリアル経由で `python_server` に送信する。`python_server` は受信した MAC アドレスをファイル名に含めて画像を保存する。

## 修正計画

1.  **`examples/image_receiver/src/main.rs` の修正:**
    *   **グローバル変数の追加:** 最後に受信した送信元 MAC アドレスを保持するための静的変数 `LAST_RECEIVED_MAC: Mutex<RefCell<Option<[u8; 6]>>>` を追加する。
    *   **`esp_now_recv_cb` の修正:** コールバック関数の冒頭で、`info` ポインタから送信元 MAC アドレス (`unsafe { (*info).src_mac }`) を取得し、`LAST_RECEIVED_MAC` を更新する。
    *   **`uart_sender_task` の修正:**
        *   `rx_channel` から画像データを受信した後、`LAST_RECEIVED_MAC` から MAC アドレスを取得する。
        *   取得した MAC アドレスを `xx:xx:xx:xx:xx:xx` 形式の文字列に変換する。
        *   `HASH:` 情報を UART に送信する *前* に、`MAC:<mac_addr_str>\n` という形式で UART に送信する。

2.  **`examples/python_server/receive_image.py` (Raspi上で実行) の修正:**
    *   `receive_and_save_jpeg` 関数に `received_mac_address = None` を追加する。
    *   メインの受信ループで、最初に `MAC:` で始まる行を待ち受け、受信したら `received_mac_address` に保存する。
    *   次に `HASH:` で始まる行を待ち受け、受信したら `received_hash_from_esp` に保存する。
    *   その後、長さプレフィックス付きの画像チャンクを受信する。
    *   JPEG の終端を検出したら、`received_mac_address` (コロンをハイフンに置換) とタイムスタンプを使ってファイル名を生成 (例: `image_YYYYMMDD_HHMMSS_ffffff_xx-xx-xx-xx-xx-xx.jpg`) し、保存する。
    *   状態リセット時に `received_mac_address` も `None` にする。

3.  **`examples/image_sender/src/main.rs` の修正:**
    *   変更は不要。

## 処理フロー図 (Mermaid)

```mermaid
sequenceDiagram
    participant Sender as image_sender (ESP32)
    participant Relay as image_receiver (ESP32)
    participant Server as python_server (Raspi)

    loop Image Transmission Cycle
        Sender->>Sender: Take Picture
        Sender->>Sender: Calculate SHA256 Hash
        Sender->>Relay: Send "HASH:<HASH>" (via ESP-NOW)
        Relay->>Relay: Receive HASH, Get Sender MAC, Store MAC globally, Store HASH globally, Clear image buffer

        loop Send Image Chunks
            Sender->>Relay: Send Image Chunk (via ESP-NOW)
            Relay->>Relay: Receive Chunk, Get Sender MAC, Store MAC globally, Append chunk to image buffer
        end

        Sender->>Relay: Send "EOF" (via ESP-NOW)
        Relay->>Relay: Receive EOF, Get Sender MAC, Store MAC globally
        Relay->>Relay: Send full image data from buffer to internal channel (tx_channel)
        Relay->>Relay: Clear image buffer

        Relay->>Server: (UART Task) Receive full image from channel
        Relay->>Server: (UART Task) Get last stored MAC globally
        Relay->>Server: (UART Task) Send "MAC:<SENDER_MAC>\n" (via Serial)
        Relay->>Server: (UART Task) Get stored HASH globally, Clear stored HASH
        Relay->>Server: (UART Task) Send "HASH:<HASH>\n" (via Serial)
        loop Send Image Chunks via UART
            Relay->>Server: (UART Task) Send Chunk (Length + Data) (via Serial)
        end

        Server->>Server: Receive "MAC:<SENDER_MAC>\n", Store MAC
        Server->>Server: Receive "HASH:<HASH>\n", Store Hash
        loop Receive Image Chunks via Serial
             Server->>Server: Receive Chunk (Length + Data), Append to buffer
        end
        Server->>Server: Detect JPEG EOI in buffer
        Server->>Server: Verify Hash (Optional)
        Server->>Server: Generate Filename (Timestamp + Stored MAC)
        Server->>Server: Save Image Buffer to File
        Server->>Server: Clear Buffer, Stored MAC, Stored Hash

        Sender->>Sender: Enter Deep Sleep
    end