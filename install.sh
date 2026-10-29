#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release
sudo install -Dm755 target/release/screenshot-uploader /usr/local/bin/screenshot-uploader

if [ ! -f /etc/screenshot-uploader.env ]; then
    read -rp "Discord webhook URL: " url
    printf 'DISCORD_WEBHOOK_URL=%s\n' "$url" | sudo tee /etc/screenshot-uploader.env >/dev/null
fi
sudo chmod 600 /etc/screenshot-uploader.env
sudo chown root:root /etc/screenshot-uploader.env

sudo install -Dm644 screenshot-uploader.service /etc/systemd/system/screenshot-uploader.service
sudo systemctl daemon-reload
sudo systemctl enable --now screenshot-uploader.service
systemctl status screenshot-uploader.service --no-pager
