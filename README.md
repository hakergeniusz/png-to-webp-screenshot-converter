# png-to-webp-screenshot-converter

A small Rust daemon that watches a screenshots directory for new PNG files, converts them to lossless WebP, uploads the converted image to a Discord webhook, and removes the originals once the upload succeeds.

## What it does

- Watches a directory for new or modified `.png` files.
- Waits for each file to finish writing before processing.
- Converts the PNG to a lossless WebP saved next to the original.
- Uploads the WebP to Discord via `DISCORD_WEBHOOK_URL`, with retries.
- Deletes the original PNG and the generated WebP after a successful upload.

## Requirements

- Rust toolchain (for building)
- A Discord webhook URL

## Configuration

- `DISCORD_WEBHOOK_URL` (required): Discord webhook to post screenshots to.
- First CLI argument (optional): directory to watch.
- `SCREENSHOT_DIR` (optional): overrides the default directory when no CLI argument is given.

If no directory is provided, the default is resolved as:
1. `$XDG_PICTURES_DIR/Screenshots` (from the environment or `~/.config/user-dirs.dirs`)
2. `$HOME/Pictures/Screenshots` (fallback)

## Run locally

```sh
export DISCORD_WEBHOOK_URL="https://discord.com/api/webhooks/..."
cargo run --release -- /path/to/screenshots
```

Or use the default XDG screenshots directory:

```sh
export DISCORD_WEBHOOK_URL="https://discord.com/api/webhooks/..."
cargo run --release
```

## Install as a systemd service

`install.sh` builds the release binary, installs it to `/usr/local/bin/screenshot-uploader`, prompts for and stores the webhook in `/etc/screenshot-uploader.env` (if missing), and enables a templated systemd unit for the current user:

```sh
./install.sh
```

The bundled unit is installed as `screenshot-uploader@.service` and started as `screenshot-uploader@<user>`. By default it uses the daemon's resolved screenshot directory; review `screenshot-uploader.service` (especially `ReadWritePaths`) if your XDG screenshots directory is not under `~/Pictures/Screenshots`.

## License

[MIT](LICENSE)
