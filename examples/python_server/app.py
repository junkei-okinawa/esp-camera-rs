import os
import time
import asyncio
import serial_asyncio
import serial
from datetime import datetime
import logging  # Ensure logging is imported
import argparse

# --- Logging Setup ---
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

# --- Settings ---
DEFAULT_SERIAL_PORT = "/dev/ttyACM0"
BAUD_RATE = 115200
IMAGE_DIR = "images_usb_async"
MAC_ADDRESS_LENGTH = 6
LENGTH_FIELD_BYTES = 4
START_MARKER = b"\xaa\xaa"
END_MARKER = b"\xbb\xbb"
HEADER_LENGTH = MAC_ADDRESS_LENGTH + LENGTH_FIELD_BYTES
FOOTER_LENGTH = len(END_MARKER)
IMAGE_TIMEOUT = 10.0  # Timeout for receiving chunks for one image (seconds)

# --- Global State ---
image_buffers = {}
last_receive_time = {}
stats = {"received_images": 0, "total_bytes": 0, "start_time": time.time()}


def ensure_dir_exists():
    if not os.path.exists(IMAGE_DIR):
        os.makedirs(IMAGE_DIR)
        logger.info(f"Created directory: {IMAGE_DIR}")


# --- Image Saving Logic ---
async def save_image(sender_mac_str, image_data):
    """Saves the received complete image data (async for potential I/O)."""
    try:
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S_%f")
        filename = f"{IMAGE_DIR}/{sender_mac_str.replace(':', '')}_{timestamp}.jpg"
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, write_file_sync, filename, image_data)

        file_size = len(image_data)
        stats["received_images"] += 1
        stats["total_bytes"] += file_size
        logger.info(
            f"Saved image from {sender_mac_str}, size: {file_size} bytes as: {filename}"
        )

        if stats["received_images"] > 0 and stats["received_images"] % 10 == 0:
            elapsed = time.time() - stats["start_time"]
            try:
                avg_size = stats["total_bytes"] / stats["received_images"]
                logger.info(
                    f"Stats: {stats['received_images']} images, avg size: {avg_size:.1f} bytes, elapsed: {elapsed:.1f}s"
                )
            except ZeroDivisionError:
                logger.info("Stats: 0 images received yet.")

    except Exception as e:
        logger.error(f"Error saving image for MAC {sender_mac_str}: {e}")


def write_file_sync(filename, data):
    """Synchronous helper function to write file data."""
    with open(filename, "wb") as f:
        f.write(data)


# --- Serial Protocol Class ---
class SerialProtocol(asyncio.Protocol):
    """Asyncio protocol to handle serial data."""

    def __init__(self, connection_lost_future: asyncio.Future):
        super().__init__()
        self.buffer = bytearray()
        self.transport = None
        # Store the future passed from the main loop
        self.connection_lost_future = connection_lost_future
        self.frame_start_time = None  # フレーム受信開始時間
        logger.info("Serial Protocol initialized.")

    def connection_made(self, transport):
        self.transport = transport
        # <<<--- [修正2] No longer need to create future here ---
        try:
            # Setting DTR might reset some devices, handle potential issues
            transport.serial.dtr = True
            logger.info(f"Serial port {transport.serial.port} opened, DTR set.")
        except IOError as e:
            logger.warning(f"Could not set DTR on {transport.serial.port}: {e}")
        # Debug log to confirm the future exists
        # logger.debug(f"connection_made: Future object ID = {id(self.connection_lost_future)}")

    def data_received(self, data):
        """Called when data is received from the serial port."""
        self.buffer.extend(data)
        self.process_buffer()  # Process the buffer immediately

    def process_buffer(self):
        """Process the buffer to find and handle complete frames."""
        global image_buffers, last_receive_time
        processed_frame = (
            False  # Flag to indicate if a frame was processed in this call
        )

        while True:  # Process all complete frames in the buffer
            # フレームレベルのタイムアウトチェック
            if self.frame_start_time and (
                time.monotonic() - self.frame_start_time > 1.0
            ):  # 例: 1秒タイムアウト
                logger.warning(
                    f"Frame timeout detected. Discarding partial frame data."
                )
                start_index_after_timeout = self.buffer.find(
                    START_MARKER, 1
                )  # 次のマーカーを探す
                if start_index_after_timeout != -1:
                    logger.warning(
                        f"Discarding {start_index_after_timeout} bytes due to frame timeout."
                    )
                    self.buffer = self.buffer[start_index_after_timeout:]
                else:
                    logger.warning(
                        "No further start marker found after frame timeout. Clearing buffer."
                    )
                    self.buffer.clear()
                self.frame_start_time = None  # タイムアウト処理後はリセット

            start_index = self.buffer.find(START_MARKER)
            if start_index == -1:
                # Keep the last potential start marker bytes if buffer is short
                if len(self.buffer) >= len(START_MARKER):
                    # 開始マーカーの一部かもしれないので、末尾を残す
                    self.buffer = self.buffer[-(len(START_MARKER) - 1) :]
                # logger.debug(f"No start marker found. Buffer len: {len(self.buffer)}")
                break  # Need more data

            # 開始マーカーが見つかったら、フレーム受信開始時間を記録
            if self.frame_start_time is None:
                self.frame_start_time = time.monotonic()

            if start_index > 0:
                discarded_data = self.buffer[:start_index]
                logger.warning(
                    f"Discarding {start_index} bytes before start marker: {discarded_data.hex()}"
                )
                self.buffer = self.buffer[start_index:]
                self.frame_start_time = (
                    time.monotonic()
                )  # マーカーを見つけたので時間リセット
                continue  # バッファを更新したのでループの最初から再試行

            if len(self.buffer) < len(START_MARKER) + HEADER_LENGTH:
                # logger.debug(f"Need more data for header. Buffer len: {len(self.buffer)}")
                break  # Need more data for header

            header_start = len(START_MARKER)
            mac_bytes = self.buffer[header_start : header_start + MAC_ADDRESS_LENGTH]
            len_bytes = self.buffer[
                header_start + MAC_ADDRESS_LENGTH : header_start + HEADER_LENGTH
            ]
            sender_mac = ":".join(f"{b:02x}" for b in mac_bytes)

            try:
                if len(len_bytes) != LENGTH_FIELD_BYTES:
                    logger.error(
                        f"Frame decode error: Incorrect length for len_bytes ({len(len_bytes)}). Discarding marker and searching next."
                    )
                    # --- 同期回復処理 ---
                    next_start = self.buffer.find(
                        START_MARKER, 1
                    )  # 現在のマーカーの次を探す
                    if next_start != -1:
                        self.buffer = self.buffer[next_start:]
                    else:
                        self.buffer.clear()  # 見つからなければクリア
                    self.frame_start_time = None  # タイムアウト処理後はリセット
                    continue
                data_len = int.from_bytes(len_bytes, byteorder="big")
            except ValueError:
                logger.error(
                    f"Frame decode error: ValueError for DataLen ({len_bytes.hex()}). Discarding marker and searching next."
                )
                # --- 同期回復処理 ---
                next_start = self.buffer.find(START_MARKER, 1)
                if next_start != -1:
                    self.buffer = self.buffer[next_start:]
                else:
                    self.buffer.clear()
                self.frame_start_time = None  # タイムアウト処理後はリセット
                continue

            # データ長チェックを少し緩める (ESP-NOW最大ペイロードサイズを考慮)
            # max_reasonable_data_len = 250 # ESP-NOWの最大ペイロード長
            max_reasonable_data_len = 512  # 念のため少し大きめに設定
            if data_len > max_reasonable_data_len:
                logger.warning(
                    f"Unreasonable data_len: {data_len} (max: {max_reasonable_data_len}). Discarding marker and searching next."
                )
                # --- 同期回復処理 ---
                next_start = self.buffer.find(START_MARKER, 1)
                if next_start != -1:
                    self.buffer = self.buffer[next_start:]
                else:
                    self.buffer.clear()
                self.frame_start_time = None  # タイムアウト処理後はリセット
                continue

            frame_end_index = (
                len(START_MARKER) + HEADER_LENGTH + data_len + FOOTER_LENGTH
            )
            if len(self.buffer) < frame_end_index:
                # logger.debug(f"Need more data for full frame. Expected: {frame_end_index}, Have: {len(self.buffer)}")
                break  # Need more data for full frame

            data_start_index = len(START_MARKER) + HEADER_LENGTH
            chunk_data = self.buffer[data_start_index : data_start_index + data_len]
            footer_start_index = data_start_index + data_len
            footer = self.buffer[footer_start_index:frame_end_index]

            if footer == END_MARKER:
                processed_frame = True
                self.frame_start_time = None  # 正常にフレームを処理したのでリセット
                # Handle HASH frame
                if data_len > 5 and chunk_data.startswith(b"HASH:"):
                    try:
                        hash_str = chunk_data[5:].decode("ascii")
                        logger.info(
                            f"Received HASH frame from {sender_mac}: {hash_str}"
                        )
                        # TODO: Store hash if validation is needed
                    except UnicodeDecodeError:
                        logger.warning(
                            f"Could not decode HASH payload from {sender_mac}: {chunk_data[5:].hex()}"
                        )
                # Handle EOF frame
                elif data_len == 4 and chunk_data == b"EOF!":
                    if sender_mac in image_buffers:
                        logger.info(
                            f"EOF frame received for {sender_mac}. Assembling image ({len(image_buffers[sender_mac])} bytes)."
                        )
                        asyncio.create_task(
                            save_image(sender_mac, bytes(image_buffers[sender_mac]))
                        )
                        del image_buffers[sender_mac]
                        if sender_mac in last_receive_time:
                            del last_receive_time[sender_mac]
                    else:
                        logger.warning(f"EOF for {sender_mac} but no buffer found.")
                # Handle regular data chunk
                elif data_len > 0:
                    if sender_mac not in image_buffers:
                        image_buffers[sender_mac] = bytearray()
                        logger.info(
                            f"Started receiving new image data from {sender_mac}"
                        )
                    image_buffers[sender_mac].extend(chunk_data)
                    last_receive_time[sender_mac] = time.monotonic()
                # Handle zero-length data frame
                elif data_len == 0:
                    logger.debug(
                        f"Frame with data_len 0 received from {sender_mac}. Ignoring."
                    )

                # フレームを処理したのでバッファから削除
                self.buffer = self.buffer[frame_end_index:]
            else:
                logger.warning(
                    f"Invalid end marker for {sender_mac} (got {footer.hex()}). Discarding marker and searching next."
                )
                # --- 同期回復処理 ---
                next_start = self.buffer.find(START_MARKER, 1)
                if next_start != -1:
                    self.buffer = self.buffer[next_start:]
                else:
                    self.buffer.clear()
                self.frame_start_time = None  # タイムアウト処理後はリセット

        # return processed_frame # この関数の戻り値は現在使われていない

    def connection_lost(self, exc):
        log_prefix = f"connection_lost ({id(self)}):"  # Add instance ID for clarity
        if exc:
            logger.error(f"{log_prefix} Serial port connection lost: {exc}")
        else:
            logger.info(f"{log_prefix} Serial port connection closed normally.")
        self.transport = None

        # <<<--- [修正3] Use the future passed during __init__ ---
        # Check if the future exists and is not already done
        # logger.debug(f"{log_prefix} Future object ID = {id(self.connection_lost_future)}")
        if self.connection_lost_future and not self.connection_lost_future.done():
            logger.info(
                f"{log_prefix} Setting connection_lost_future result/exception."
            )
            if exc:
                try:
                    self.connection_lost_future.set_exception(exc)
                except asyncio.InvalidStateError:
                    logger.warning(
                        f"{log_prefix} Future was already set/cancelled when trying to set exception."
                    )
            else:
                try:
                    self.connection_lost_future.set_result(True)
                except asyncio.InvalidStateError:
                    logger.warning(
                        f"{log_prefix} Future was already set/cancelled when trying to set result."
                    )
        else:
            state = (
                "None"
                if not self.connection_lost_future
                else (
                    "Done"
                    if self.connection_lost_future.done()
                    else "Exists but not done?"
                )
            )
            logger.warning(
                f"{log_prefix} connection_lost called but future state is: {state}."
            )


# --- Timeout Checker Task ---
async def check_timeouts():
    """Periodically check for timed out image buffers."""
    global image_buffers, last_receive_time
    while True:
        try:
            await asyncio.sleep(IMAGE_TIMEOUT)
            current_time = time.monotonic()
            timed_out_macs = [
                mac
                for mac, last_time in list(last_receive_time.items())
                if current_time - last_time > IMAGE_TIMEOUT
            ]
            for mac in timed_out_macs:
                logger.warning(
                    f"Timeout waiting for data from {mac}. Discarding buffer ({len(image_buffers.get(mac, b''))} bytes)."
                )
                if mac in image_buffers:
                    del image_buffers[mac]
                if mac in last_receive_time:
                    del last_receive_time[mac]
        except asyncio.CancelledError:
            logger.info("Timeout checker task cancelled.")
            break
        except Exception as e:
            logger.exception(f"Error in timeout checker: {e}")


# --- Main Application Logic ---
async def main(port, baud):
    """Main asynchronous function."""
    ensure_dir_exists()
    logger.info("Starting Async USB CDC Image Receiver")
    logger.info(f"Images will be saved to: {os.path.abspath(IMAGE_DIR)}")

    loop = asyncio.get_running_loop()
    timeout_task = loop.create_task(check_timeouts())

    while True:  # Reconnection loop
        transport = None
        active_protocol = None
        # <<<--- [修正4] Create the Future in the main loop ---
        connection_lost_future = loop.create_future()
        # logger.debug(f"main loop: Created Future object ID = {id(connection_lost_future)}")

        try:
            logger.info(f"Attempting to connect to {port} at {baud} baud...")

            # <<<--- [修正5] Pass the created Future via the factory ---
            # The lambda creates a protocol instance and passes the future to its __init__
            protocol_factory = lambda: SerialProtocol(connection_lost_future)

            # serial_asyncio creates the protocol instance using the factory
            transport, active_protocol = await serial_asyncio.create_serial_connection(
                loop, protocol_factory, port, baudrate=baud
            )
            logger.info("Connection established.")
            # active_protocol should now hold the instance created by the factory

            # <<<--- [修正6] No need to retrieve the future here, just await the one we created ---
            logger.info("Monitoring connection (awaiting future)...")
            await connection_lost_future
            # Execution continues here after connection_lost sets the future result/exception
            logger.info("Connection lost signaled (future completed).")

        except serial.SerialException as e:
            logger.error(f"Serial connection error: {e}")
            # If connection failed, the future might not be set by connection_lost
            # Set it here to prevent the loop from waiting indefinitely on await sleep(5)
            if not connection_lost_future.done():
                logger.warning(
                    "Setting future exception due to SerialException during connection."
                )
                connection_lost_future.set_exception(e)
        except asyncio.CancelledError:
            logger.info("Main task cancelled during connection/monitoring.")
            # Ensure future is cancelled if await was interrupted
            if connection_lost_future and not connection_lost_future.done():
                connection_lost_future.cancel("Main task cancelled")
            break  # Exit the while loop
        except Exception as e:
            logger.exception(f"Error during connection or monitoring: {e}")
            # Ensure the future is set if an unexpected error occurs,
            # otherwise the loop might hang.
            if connection_lost_future and not connection_lost_future.done():
                try:
                    logger.warning(
                        f"Setting future exception due to unexpected error: {e}"
                    )
                    connection_lost_future.set_exception(e)
                except asyncio.InvalidStateError:
                    pass  # Future was already done/cancelled
        finally:
            # Close transport if it exists and is not already closing
            if transport and not transport.is_closing():
                logger.info("Closing transport in finally block.")
                transport.close()
            # Clear references for the next iteration
            transport = None
            active_protocol = None

        # Check loop status before sleeping
        if not loop.is_running():
            logger.warning("Event loop is not running. Exiting reconnection loop.")
            break

        # Wait before retrying connection
        logger.info(f"Waiting {5} seconds before retrying connection...")
        try:
            # Log if the previous connection ended with an error
            if connection_lost_future.done() and connection_lost_future.exception():
                logger.info(
                    f"Previous connection ended with error: {connection_lost_future.exception()}"
                )
            await asyncio.sleep(5)
        except asyncio.CancelledError:
            logger.info("Retry delay cancelled. Exiting reconnection loop.")
            break  # Exit the while loop

    # Cleanup
    logger.info("Shutting down timeout task...")
    timeout_task.cancel()
    try:
        await timeout_task
    except asyncio.CancelledError:
        pass  # Expected cancellation
    logger.info("Application finished.")


# --- Entry Point ---
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Async receive images via USB CDC.")
    parser.add_argument(
        "-p",
        "--port",
        default=DEFAULT_SERIAL_PORT,
        help=f"Serial port (default: {DEFAULT_SERIAL_PORT})",
    )
    parser.add_argument(
        "-b",
        "--baud",
        type=int,
        default=BAUD_RATE,
        help=f"Baud rate (default: {BAUD_RATE})",
    )
    args = parser.parse_args()

    try:
        asyncio.run(main(args.port, args.baud))
    except KeyboardInterrupt:
        logger.info("Exiting due to KeyboardInterrupt.")
