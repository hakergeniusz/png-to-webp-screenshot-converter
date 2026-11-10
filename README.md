# png-to-webp-screenshot-converter

A small Rust daemon that watches a screenshots directory for new PNG files, converts them to lossless WebP, uploads the converted image to a Discord webhook, and removes the originals once the upload succeeds.

## What it does

- Watches a directory (default: `/home/hakergeniusz/Pictures/Screenshots`) for new or modified `.png` files.
- Waits for each file to finish writing before processing.
- Converts the PNG to a lossless WebP saved next to the original.
- Uploads the WebP to Discord via `DISCORD_WEBHOOK_URL`, with retries.
- Deletes the original PNG and the generated WebP after a successful upload.

## Requirements

- Rust toolchain (for building)
- A Discord webhook URL

## Configuration

- `DISCORD_WEBHOOK_URL` (required): Discord webhook to post screenshots to.
- First CLI argument (optional): directory to watch. Defaults to `/home/hakergeniusz/Pictures/Screenshots`.

## Run locally

```sh
export DISCORD_WEBHOOK_URL="https://discord.com/api/webhooks/..."
cargo run --release -- /path/to/screenshots
```

## Install as a systemd service

`install.sh` builds the release binary, installs it to `/usr/local/bin/screenshot-uploader`, prompts for and stores the webhook in `/etc/screenshot-uploader.env` (if missing), and enables `screenshot-uploader.service`:

```sh
./install.sh
```

The bundled service unit runs as `hakergeniusz` and watches `/home/hakergeniusz/Pictures/Screenshots`. Edit `screenshot-uploader.service` and the paths in `install.sh` if you need different locations or user.

## License

[MIT](LICENSE)
