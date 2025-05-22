# Python USB CDC Image Receiver サービスの systemd 常駐化手順

## 前提条件
リポジトリのルートディレクトリで以下の手順を実行しアプリケーションが動作することを確認しておく

```bash
uv venv # 仮想環境作成
source .venv/bin/activate # 仮想環境有効化
uv sync # 依存関係のインストール
.venv/bin/python app.py
```

## systemd サービスとして常駐起動する手順

1. サービスファイルの配置
   
   `python_server/systemd/python_server.service` を `/etc/systemd/system/` にコピーします。
   
   ```bash
   sudo cp /Users/junkei/Documents/esp_learning/esp-camera-rs3/examples/python_server/systemd/python_server.service /etc/systemd/system/
   ```

2. サービスファイルの編集
   
   - `<user_name>` を実行ユーザー名に書き換えてください。
   - `Group=<group_name>` は必要な場合のみグループ名に書き換え、不要なら削除してください。

3. systemd のリロード
   
   ```bash
   sudo systemctl daemon-reload
   ```

4. サービスの有効化と起動
   
   ```bash
   sudo systemctl enable python_server
   sudo systemctl start python_server
   ```

5. ステータス確認
   
   ```bash
   sudo systemctl status python_server
   ```

---

- サービスのログは `journalctl -u python_server` で確認できます。
- サービスの停止は `sudo systemctl stop python_server` で行えます。
