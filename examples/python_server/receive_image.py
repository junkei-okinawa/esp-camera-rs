import serial
import struct
import time
import datetime
import argparse
import sys
import hashlib # Add hashlib for SHA256

# JPEG終了マーカー
JPEG_EOI = b'\xFF\xD9'

def read_exact(ser, num_bytes):
    """指定されたバイト数を確実に読み取る"""
    data = bytearray()
    start_time = time.time()
    while len(data) < num_bytes:
        read_byte = ser.read(1)
        if not read_byte:
            # タイムアウトチェック (例: 5秒)
            if time.time() - start_time > 5:
                print(f"Timeout reading exact {num_bytes} bytes. Got {len(data)} bytes.")
                return None
            time.sleep(0.001) # 少し待つ
            continue
        data.extend(read_byte)
        start_time = time.time() # データ受信したらタイムアウトタイマーリセット
    return bytes(data)

def receive_and_save_jpeg(serial_port, baud_rate, output_dir="."):
    """
    シリアルポートから画像データチャンクを受信し、JPEGファイルとして保存する。
    最初に MAC:<mac_address> 形式の行を受信し、
    次に HASH:<sha256_hex> 形式の行を受信し、
    最後に各チャンクの前にデータ長(u16リトルエンディアン)を受信する想定。
    """
    received_mac_address = None # 追加: 受信したMACアドレスを保持
    received_hash_from_esp = None
    image_buffer = bytearray()
    # receiving_image = False # 状態管理変数 state に置き換え
    last_data_time = time.time()
    state = "WAITING_MAC" # 状態管理: WAITING_MAC, WAITING_HASH, RECEIVING_CHUNKS
    try:
        ser = serial.Serial(serial_port, baud_rate, timeout=1) # タイムアウトを1秒に短縮
        print(f"Opened serial port {serial_port} at {baud_rate} baud.")
    except serial.SerialException as e:
        print(f"Error opening serial port {serial_port}: {e}")
        sys.exit(1)

    print("Waiting for MAC marker...")

    try:
        while True:
            # --- State Machine ---
            if state == "WAITING_MAC":
                line_bytes = ser.readline()
                if not line_bytes: # タイムアウト
                    if time.time() - last_data_time > 10:
                        print("Still waiting for MAC...")
                        last_data_time = time.time()
                    continue

                line = line_bytes.decode('utf-8', errors='ignore').strip()
                if line.startswith("MAC:"):
                    received_mac_address = line[4:]
                    print(f"Received MAC: {received_mac_address}")
                    state = "WAITING_HASH" # 次の状態へ
                    last_data_time = time.time()
                    print("Waiting for HASH marker...")
                elif line: # Print other unexpected lines
                    print(f"Received unexpected line while waiting for MAC: {line}")
                continue # Continue waiting for MAC

            elif state == "WAITING_HASH":
                line_bytes = ser.readline()
                if not line_bytes: # タイムアウト
                    if time.time() - last_data_time > 10:
                        print("Still waiting for HASH...")
                        last_data_time = time.time()
                    continue

                line = line_bytes.decode('utf-8', errors='ignore').strip()
                if line.startswith("HASH:"):
                    received_hash_from_esp = line[5:]
                    print(f"Received HASH: {received_hash_from_esp}")
                    image_buffer = bytearray() # Reset buffer after receiving hash
                    state = "RECEIVING_CHUNKS" # 次の状態へ
                    last_data_time = time.time()
                    print("Waiting for image data chunks...")
                elif line: # Print other unexpected lines
                    print(f"Received unexpected line while waiting for HASH: {line}")
                    # MACは受信済みなのでリセット
                    print("Resetting state. Waiting for MAC again...")
                    received_mac_address = None
                    ser.reset_input_buffer() # Clear input buffer before resetting state
                    state = "WAITING_MAC"
                continue # Continue waiting for HASH or start receiving chunks

            elif state == "RECEIVING_CHUNKS":
                # --- Receive image chunks ---
                # (以下のチャンク受信ロジックはこの状態でのみ実行される)
                pass # 既存のチャンク受信ロジックへ続く
            else:
                print(f"Error: Unknown state '{state}'. Resetting.")
                received_mac_address = None
                received_hash_from_esp = None
                image_buffer = bytearray()
                ser.reset_input_buffer() # Clear input buffer before resetting state
                state = "WAITING_MAC"
                print("-" * 20)
                print("Waiting for MAC marker...")
                continue
            # --- Receive image chunks ---
            # チャンク長の読み取り (確実に2バイト読み取る)
            len_bytes = read_exact(ser, 2)
            if len_bytes is None:
                # タイムアウト処理 (チャンク受信中)
                if time.time() - last_data_time > 10: # 10秒データが来なければリセット
                    print("Timeout waiting for chunk length. Resetting state.")
                    received_mac_address = None # MACもリセット
                    received_hash_from_esp = None
                    image_buffer = bytearray()
                    ser.reset_input_buffer() # Clear input buffer before resetting state
                    state = "WAITING_MAC" # 状態をリセット
                    print("-" * 20)
                    print("Waiting for MAC marker...")
                continue

            last_data_time = time.time() # Update last data time

            # リトルエンディアンでu16として解釈
            try:
                chunk_len = struct.unpack('<H', len_bytes)[0]
            except struct.error as e:
                print(f"Error unpacking length: {e}, received bytes: {len_bytes.hex()}")
                # 同期ずれの可能性があるのでリセット (チャンク受信中)
                ser.read(ser.in_waiting) # バッファを読み捨てる
                received_mac_address = None # MACもリセット
                received_hash_from_esp = None
                image_buffer = bytearray()
                ser.reset_input_buffer() # Clear input buffer before resetting state
                state = "WAITING_MAC" # 状態をリセット
                print("-" * 20)
                print("Waiting for MAC marker...")
                continue

            # 不正な長さチェック (ESP32側は最大250バイトのはず)
            if chunk_len == 0 or chunk_len > 250:
                print(f"Warning: Received invalid chunk length: {chunk_len}. Resetting state.")
                ser.read(ser.in_waiting) # バッファを読み捨てる
                received_mac_address = None # MACもリセット
                received_hash_from_esp = None
                image_buffer = bytearray()
                ser.reset_input_buffer() # Clear input buffer before resetting state
                state = "WAITING_MAC" # 状態をリセット
                print("-" * 20)
                print("Waiting for MAC marker...")
                continue

            # データチャンクの読み取り (確実にchunk_lenバイト読み取る)
            chunk_data = read_exact(ser, chunk_len)
            if chunk_data is None:
                print(f"Warning: Timeout waiting for chunk data (expected {chunk_len} bytes). Resetting state.")
                received_mac_address = None # MACもリセット
                received_hash_from_esp = None
                image_buffer = bytearray()
                ser.reset_input_buffer() # Clear input buffer before resetting state
                state = "WAITING_MAC" # 状態をリセット
                print("-" * 20)
                print("Waiting for MAC marker...")
                continue

            # 最初のチャンクを受信したら受信開始フラグを立てる
            # チャンクデータをバッファに追加 (receiving_image フラグは不要になった)
            image_buffer.extend(chunk_data)
            # print(f"Received chunk: {len(chunk_data)} bytes. Total buffer: {len(image_buffer)} bytes") # デバッグ用

            # JPEG開始マーカー(SOI)のチェックは不要になった (HASH受信後にバッファクリアするため)
            # if not receiving_image: ... (削除)
            # else: ... (削除)

            # JPEG終了マーカー(EOI)を検出 (インデント修正)
            eoi_index = image_buffer.find(JPEG_EOI)
            if eoi_index != -1:
                print(f"JPEG End Of Image (EOI) detected.")
                # EOIまでのデータを取得
                jpeg_data = bytes(image_buffer[:eoi_index + len(JPEG_EOI)]) # Convert to bytes

                # --- Hash Verification ---
                calculated_hash = hashlib.sha256(jpeg_data).hexdigest()
                print(f"Calculated SHA256: {calculated_hash}")
                if received_hash_from_esp:
                    if received_hash_from_esp == calculated_hash:
                        print("Hash verification successful!")
                    else:
                        print(f"!!! HASH MISMATCH !!! Received: {received_hash_from_esp}, Calculated: {calculated_hash}")
                else:
                    print("Warning: No hash received from ESP32 to compare.")

                # --- ファイルに保存 ---
                timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S_%f")
                # ファイル名にMACアドレスを含める (コロンをハイフンに置換)
                mac_suffix = received_mac_address.replace(':', '-') if received_mac_address else "UNKNOWN_MAC"
                filename = f"image_{timestamp}_{mac_suffix}.jpg"
                filepath = f"{output_dir}/{filename}"
                try:
                    with open(filepath, 'wb') as f:
                        f.write(jpeg_data)
                    print(f"Saved image to {filepath} ({len(jpeg_data)} bytes)")
                except IOError as e:
                    print(f"Error saving file {filepath}: {e}")

                # バッファと受信状態をリセット
                received_mac_address = None # MACアドレスもリセット
                received_hash_from_esp = None # Reset hash for next image
                image_buffer = bytearray()
                ser.reset_input_buffer() # Clear input buffer before resetting state
                state = "WAITING_MAC" # 状態をリセット
                print("-" * 20)
                print("Waiting for MAC marker...")


    except KeyboardInterrupt:
        print("\nExiting...")
    except Exception as e:
        print(f"\nAn error occurred: {e}")
    finally:
        if 'ser' in locals() and ser.is_open:
            ser.close()
            print(f"Closed serial port {serial_port}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Receive JPEG image data over serial port.")
    parser.add_argument('port', help='Serial port name (e.g., /dev/serial0, COM3)')
    parser.add_argument('-b', '--baud', type=int, default=115200, help='Baud rate (default: 115200)')
    parser.add_argument('-o', '--output', default=".", help='Output directory for saved images (default: current directory)')
    args = parser.parse_args()

    receive_and_save_jpeg(args.port, args.baud, args.output)
