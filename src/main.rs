use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use image::{ImageFormat, ImageReader};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use reqwest::blocking::{multipart, Client};
use sha2::{Digest, Sha256};
use std::io::Cursor;

const UPLOAD_ATTEMPTS: u32 = 3;

fn default_screenshot_dir() -> PathBuf {
    if let Some(dir) = env::var_os("SCREENSHOT_DIR") {
        return PathBuf::from(dir);
    }

    let pictures_dir = xdg_pictures_dir().unwrap_or_else(|| {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join("Pictures")
    });
    pictures_dir.join("Screenshots")
}

fn xdg_pictures_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("XDG_PICTURES_DIR") {
        return Some(expand_home(PathBuf::from(dir)));
    }

    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let user_dirs = config_home.join("user-dirs.dirs");
    let contents = fs::read_to_string(user_dirs).ok()?;

    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("XDG_PICTURES_DIR=") {
            continue;
        }
        let value = line.trim_start_matches("XDG_PICTURES_DIR=").trim();
        let value = value.strip_prefix('"').unwrap_or(value);
        let value = value.strip_suffix('"').unwrap_or(value);
        if value.is_empty() {
            return None;
        }
        return Some(expand_home(PathBuf::from(value)));
    }

    None
}

fn expand_home(path: PathBuf) -> PathBuf {
    let Some(path_str) = path.to_str() else {
        return path;
    };
    if path_str == "$HOME" {
        return env::var_os("HOME").map(PathBuf::from).unwrap_or(path);
    }
    if let Some(rest) = path_str.strip_prefix("$HOME/") {
        return env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or(path);
    }
    path
}

fn main() -> Result<()> {
    let webhook = env::var("DISCORD_WEBHOOK_URL")
        .context("Set the DISCORD_WEBHOOK_URL environment variable to your Discord webhook URL")?;
    let dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_screenshot_dir);

    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let _ = tx.send(res);
    })?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("cannot watch {}", dir.display()))?;

    eprintln!("Watching {} for new PNG screenshots...", dir.display());

    for res in rx {
        let event = match res {
            Ok(e) => e,
            Err(e) => {
                eprintln!("watch error: {e}");
                continue;
            }
        };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            continue;
        }
        for path in event.paths {
            if !is_png(&path) {
                continue;
            }
            // Events are processed one by one. Duplicate events for a file that was
            // already handled find it gone and are skipped inside process().
            if let Err(e) = process(&path, &client, &webhook) {
                eprintln!("[{}] error: {e:#}", path.display());
            }
        }
    }
    Ok(())
}

fn is_png(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map_or(false, |e| e.eq_ignore_ascii_case("png"))
}

/// Wait until the file size stops changing (the screenshot tool finished writing).
fn wait_until_stable(path: &Path) -> Result<()> {
    let start = Instant::now();
    let mut last = None;
    loop {
        let len = fs::metadata(path)?.len();
        if len > 0 && last == Some(len) {
            return Ok(());
        }
        last = Some(len);
        if start.elapsed() > Duration::from_secs(30) {
            bail!("file never stabilised");
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn process(path: &Path, client: &Client, webhook: &str) -> Result<()> {
    if !path.exists() {
        return Ok(()); // already handled
    }
    wait_until_stable(path)?;

    // Read everything we need from the original PNG up front.
    let png = fs::read(path)?;
    let meta = fs::metadata(path)?;
    let created: DateTime<Local> = meta
        .created()
        .or_else(|_| meta.modified())
        .unwrap_or_else(|_| SystemTime::now())
        .into();
    let sha = format!("{:x}", Sha256::digest(&png));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("screenshot.png")
        .to_string();

    // 1. PNG -> WebP (lossless), written atomically next to the original.
    let img = ImageReader::with_format(Cursor::new(&png), ImageFormat::Png)
        .decode()
        .context("PNG decode failed")?;
    let img = if img.color().has_alpha() {
        image::DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        image::DynamicImage::ImageRgb8(img.to_rgb8())
    };
    let webp_path = path.with_extension("webp");
    let tmp_path = path.with_extension("webp.tmp");
    img.save_with_format(&tmp_path, ImageFormat::WebP)
        .context("WebP encode failed")?;
    fs::rename(&tmp_path, &webp_path)?;

    // 2. Delete the PNG (we still hold its bytes in memory).
    fs::remove_file(path)?;
    eprintln!("[{file_name}] converted -> {}", webp_path.display());

    // 3 + 4. Upload the original PNG with its SHA256 and creation date.
    let content = format!(
        "**{file_name}**\nSHA256: `{sha}`\nCreated: {}",
        created.format("%Y-%m-%d %H:%M:%S %Z")
    );

    let mut last_err = None;
    for attempt in 1..=UPLOAD_ATTEMPTS {
        match upload(client, webhook, &content, &file_name, &png) {
            Ok(()) => {
                eprintln!("[{file_name}] uploaded (sha256 {sha})");
                return Ok(());
            }
            Err(e) => {
                eprintln!("[{file_name}] upload attempt {attempt} failed: {e:#}");
                last_err = Some(e);
                thread::sleep(Duration::from_secs(2 * attempt as u64));
            }
        }
    }

    // The PNG is already deleted, so don't lose the original: park it in a subfolder.
    let fail_dir = path.parent().unwrap_or(Path::new(".")).join("upload-failed");
    fs::create_dir_all(&fail_dir)?;
    let saved = fail_dir.join(&file_name);
    fs::write(&saved, &png)?;
    bail!(
        "upload failed ({}); original PNG saved to {}",
        last_err.map(|e| format!("{e:#}")).unwrap_or_default(),
        saved.display()
    );
}

fn upload(client: &Client, webhook: &str, content: &str, name: &str, png: &[u8]) -> Result<()> {
    let part = multipart::Part::bytes(png.to_vec())
        .file_name(name.to_string())
        .mime_str("image/png")?;
    let form = multipart::Form::new()
        .text("content", content.to_string())
        .part("files[0]", part);
    let resp = client.post(webhook).multipart(form).send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        bail!("Discord returned {status}: {body}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        saved: Vec<(String, Option<OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(keys: &[&str]) -> Self {
            let lock = env_lock().lock().expect("env lock poisoned");
            let saved = keys
                .iter()
                .map(|k| (k.to_string(), env::var_os(k)))
                .collect();
            Self { saved, _lock: lock }
        }

        fn set(&self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
            env::set_var(key, value);
        }

        fn remove(&self, key: &str) {
            env::remove_var(key);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(v) => env::set_var(&key, v),
                    None => env::remove_var(&key),
                }
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = env::temp_dir().join(format!("png-to-webp-tests-{name}-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn is_png_matches_case_insensitively() {
        assert!(is_png(Path::new("shot.PNG")));
        assert!(is_png(Path::new("/tmp/shot.png")));
        assert!(!is_png(Path::new("shot.webp")));
        assert!(!is_png(Path::new("shot")));
    }

    #[test]
    fn expand_home_replaces_home_prefix() {
        let guard = EnvGuard::new(&["HOME"]);
        guard.set("HOME", "/tmp/test-home");

        assert_eq!(expand_home(PathBuf::from("$HOME")), PathBuf::from("/tmp/test-home"));
        assert_eq!(
            expand_home(PathBuf::from("$HOME/Pictures")),
            PathBuf::from("/tmp/test-home/Pictures")
        );
        assert_eq!(
            expand_home(PathBuf::from("/absolute/path")),
            PathBuf::from("/absolute/path")
        );
    }

    #[test]
    fn xdg_pictures_dir_prefers_env_var() {
        let guard = EnvGuard::new(&["XDG_PICTURES_DIR", "XDG_CONFIG_HOME", "HOME"]);
        guard.set("HOME", "/tmp/test-home");
        guard.set("XDG_PICTURES_DIR", "$HOME/MyPictures");

        assert_eq!(
            xdg_pictures_dir(),
            Some(PathBuf::from("/tmp/test-home/MyPictures"))
        );
    }

    #[test]
    fn xdg_pictures_dir_reads_user_dirs_file() {
        let guard = EnvGuard::new(&["XDG_PICTURES_DIR", "XDG_CONFIG_HOME", "HOME"]);
        guard.remove("XDG_PICTURES_DIR");
        let home = temp_dir("home");
        let config = home.join(".config");
        fs::create_dir_all(&config).expect("config dir");
        fs::write(
            config.join("user-dirs.dirs"),
            "XDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Images\"\n",
        )
        .expect("write user-dirs");
        guard.set("HOME", &home);
        guard.remove("XDG_CONFIG_HOME");

        assert_eq!(xdg_pictures_dir(), Some(home.join("Images")));
    }

    #[test]
    fn default_screenshot_dir_prefers_screenshot_dir_env() {
        let guard = EnvGuard::new(&["SCREENSHOT_DIR", "XDG_PICTURES_DIR", "XDG_CONFIG_HOME", "HOME"]);
        guard.set("SCREENSHOT_DIR", "/tmp/custom-shots");

        assert_eq!(default_screenshot_dir(), PathBuf::from("/tmp/custom-shots"));
    }

    #[test]
    fn default_screenshot_dir_uses_xdg_pictures_fallback() {
        let guard = EnvGuard::new(&["SCREENSHOT_DIR", "XDG_PICTURES_DIR", "XDG_CONFIG_HOME", "HOME"]);
        guard.remove("SCREENSHOT_DIR");
        guard.remove("XDG_PICTURES_DIR");
        guard.set("HOME", "/tmp/test-home");
        guard.set("XDG_CONFIG_HOME", "/tmp/test-home/config-missing");

        assert_eq!(
            default_screenshot_dir(),
            PathBuf::from("/tmp/test-home/Pictures/Screenshots")
        );
    }
}
