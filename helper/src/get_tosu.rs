// downloads tosu's latest release from github, checked against github's sha256

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{log, tosu, Bridge, State, VERSION};

const LATEST: &str = "https://api.github.com/repos/tosuapp/tosu/releases/latest";
const DOWNLOADS: &str = "https://github.com/tosuapp/tosu/releases/download/";
// the zips are 40-50 MB, tosu itself about twice that
const MAX_ZIP: u64 = 300 << 20;
const MAX_EXE: u64 = 600 << 20;

#[derive(Default)]
pub(crate) struct GetState {
    // "downloading", "unpacking" or "failed"
    pub step: Option<&'static str>,
    pub done: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

pub(crate) fn exe_name() -> &'static str {
    if cfg!(windows) {
        "tosu.exe"
    } else {
        "tosu"
    }
}

pub(crate) fn install_dir(b: &Bridge) -> PathBuf {
    b.env.data_dir.join("tosu")
}

pub(crate) fn public(b: &Bridge, s: &State) -> Value {
    let t = &s.tosu;
    let g = &s.get_tosu;
    let offer = b.env.tosu_can_start && !t.connected && t.found == Some(false) && t.launch != Some("with-osu");
    if !offer && g.step.is_none() {
        return Value::Null;
    }
    let percent = g.total.filter(|t| *t > 0).map(|t| (g.done * 100 / t).min(100));
    json!({ "step": g.step, "percent": percent, "error": g.error })
}

pub(crate) fn start(b: &Arc<Bridge>) {
    {
        let mut s = b.st();
        if !b.env.tosu_can_start || s.tosu.connected || matches!(s.get_tosu.step, Some("downloading" | "unpacking")) {
            return;
        }
        s.get_tosu = GetState { step: Some("downloading"), ..Default::default() };
    }
    b.broadcast();
    let b2 = b.clone();
    b.spawn(async move {
        match get(&b2).await {
            Ok(exe) => {
                log!("tosu installed: {}", exe.display());
                {
                    let mut s = b2.st();
                    s.get_tosu = GetState::default();
                    s.tosu.found = Some(true);
                    if s.tosu.launch == Some("missing") {
                        s.tosu.launch = None;
                    }
                }
                tosu::set_tosu_autostart(&b2, true).await;
                tosu::refresh_dashboard(&b2);
            }
            Err(e) => {
                log!("getting tosu failed: {e}");
                let mut s = b2.st();
                s.get_tosu.step = Some("failed");
                s.get_tosu.error = Some(e);
            }
        }
        b2.broadcast();
    });
}

fn progress(b: &Arc<Bridge>, done: u64, total: Option<u64>) {
    {
        let mut s = b.st();
        s.get_tosu.done = done;
        s.get_tosu.total = total;
    }
    b.broadcast();
}

async fn get(b: &Arc<Bridge>) -> Result<PathBuf, String> {
    let net = |e: reqwest::Error| crate::auth::net_message(&e);
    // github sends downloads on to its file host, and only there
    let client = reqwest::Client::builder()
        .user_agent(format!("mania-bridge/{VERSION}"))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            let host = a.url().host_str().unwrap_or("");
            let github = host == "github.com" || host.ends_with(".githubusercontent.com");
            if a.previous().len() < 5 && a.url().scheme() == "https" && github {
                a.follow()
            } else {
                a.stop()
            }
        }))
        .build()
        .map_err(|e| e.to_string())?;

    let res = client.get(LATEST).header("accept", "application/vnd.github+json").timeout(Duration::from_secs(20)).send().await.map_err(net)?;
    if !res.status().is_success() {
        return Err(format!("GitHub answered {} for tosu's latest release.", res.status().as_u16()));
    }
    let release: Value = res.json().await.map_err(|e| e.to_string())?;
    let want = if cfg!(windows) { "tosu-windows-" } else { "tosu-linux-" };
    let asset = release
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(Value::as_str).is_some_and(|n| n.starts_with(want) && n.ends_with(".zip"))))
        .ok_or("tosu's latest release has no download for this system.")?;
    let url = asset.get("browser_download_url").and_then(Value::as_str).filter(|u| u.starts_with(DOWNLOADS)).ok_or("tosu's download isn't on its GitHub.")?;
    let sha = asset
        .get("digest")
        .and_then(Value::as_str)
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| d.len() == 64 && d.bytes().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_lowercase)
        .ok_or("tosu's download has no checksum to check it against.")?;
    let size = asset.get("size").and_then(Value::as_u64);
    if size.is_some_and(|s| s > MAX_ZIP) {
        return Err("tosu's download is far bigger than it should be.".into());
    }
    let version = release.get("tag_name").and_then(Value::as_str).unwrap_or("?");
    log!("getting tosu {version}");

    let mut res = client.get(url).timeout(Duration::from_secs(600)).send().await.map_err(net)?;
    if !res.status().is_success() {
        return Err(format!("GitHub answered {} for tosu's download.", res.status().as_u16()));
    }
    let total = res.content_length().or(size);
    let mut zip = Vec::with_capacity(total.unwrap_or(0).min(MAX_ZIP) as usize);
    let mut hash = Sha256::new();
    let mut last = 0;
    while let Some(chunk) = res.chunk().await.map_err(net)? {
        hash.update(&chunk);
        zip.extend_from_slice(&chunk);
        if zip.len() as u64 > MAX_ZIP {
            return Err("tosu's download is far bigger than it should be.".into());
        }
        if (zip.len() - last) as u64 * 100 >= total.unwrap_or(u64::MAX).max(1) {
            last = zip.len();
            progress(b, zip.len() as u64, total);
        }
    }
    if hex(&hash.finalize()) != sha {
        return Err("The download didn't match tosu's checksum, so it wasn't used.".into());
    }

    {
        let mut s = b.st();
        s.get_tosu.step = Some("unpacking");
    }
    b.broadcast();
    let dir = install_dir(b);
    let exe = tokio::task::spawn_blocking(move || unpack(&zip, &dir)).await.map_err(|e| e.to_string())??;
    quiet_dashboard(&exe);
    Ok(exe)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unpack(zip: &[u8], dir: &Path) -> Result<PathBuf, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| format!("tosu's zip can't be read: {e}"))?;
    let index = (0..archive.len())
        .find(|i| archive.by_index(*i).ok().and_then(|f| f.enclosed_name()).is_some_and(|p| p.file_name().is_some_and(|n| n == exe_name())))
        .ok_or("tosu's zip has no tosu in it.")?;
    let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
    if file.size() > MAX_EXE {
        return Err("tosu in the zip is far bigger than it should be.".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let exe = dir.join(exe_name());
    let part = dir.join(format!("{}.part", exe_name()));
    let result = (|| {
        let mut out = std::fs::File::create(&part)?;
        std::io::copy(&mut (&mut file).take(MAX_EXE + 1), &mut out)?;
        out.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&part, &exe)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(format!("tosu couldn't be unpacked: {e}"));
    }
    Ok(exe)
}

// otherwise tosu opens its dashboard in the browser on every start
fn quiet_dashboard(exe: &Path) {
    let config = if cfg!(target_os = "linux") {
        crate::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| crate::home().join(".config")).join("tosu")
    } else {
        exe.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let file = config.join("tosu.env");
    if file.exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&config).and_then(|()| std::fs::write(&file, "OPEN_DASHBOARD_ON_STARTUP=false\n")) {
        log!("couldn't write {}: {e}", file.display());
    }
}
