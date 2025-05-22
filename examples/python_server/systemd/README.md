# Python USB CDC Image Receiver Service - systemd Setup

## Prerequisites
Before setting up as a service, make sure the application works by running the following commands in the repository root:

```bash
uv venv # Create virtual environment
source .venv/bin/activate # Activate virtual environment
uv sync # Install dependencies
.venv/bin/python app.py
```

## How to Run as a systemd Service

1. Copy the service file
   
   Copy `python_server/systemd/python_server.service` to `/etc/systemd/system/`:
   
   ```bash
   sudo cp /Users/junkei/Documents/esp_learning/esp-camera-rs3/examples/python_server/systemd/python_server.service /etc/systemd/system/
   ```

2. Edit the service file
   
   - Replace `<user_name>` with the user that should run the service.
   - If you need to specify a group, replace `<group_name>` with the group name. If not needed, remove the `Group` line.

3. Reload systemd
   
   ```bash
   sudo systemctl daemon-reload
   ```

4. Enable and start the service
   
   ```bash
   sudo systemctl enable python_server
   sudo systemctl start python_server
   ```

5. Check service status
   
   ```bash
   sudo systemctl status python_server
   ```

---

- To view logs: `journalctl -u python_server`
- To stop the service: `sudo systemctl stop python_server`
