use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use reqwest::header::CACHE_CONTROL;
use serde_json::{json, Value};

use crate::{log, Bridge, VERSION};

pub type Progress = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;

pub trait Updater: Send + Sync {
    // whether this copy can replace itself (an installed one, not a dev build)
    fn can_install(&self) -> bool;
    // on windows the installer takes over and this never returns
    fn install(&self, url: String, progress: Progress) -> BoxFuture<'static, Result<(), String>>;
}

pub const LATEST_FILE: &str = "/hashi/latest.json";
const DOWNLOAD_PAGE: &str = "/hashi";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_BYTES: usize = 64 * 1024;

type Version = (u32, u32, u32);

#[derive(Default)]
pub(crate) struct UpdateState {
    pub available: Option<String>,
    // "later" hides it until the next start, or a newer one than that
    pub dismissed: bool,
    // installing: "downloading", "installing" or "failed"
    pub step: Option<&'static str>,
    pub done: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

impl UpdateState {
    pub fn public(&self, can_install: bool) -> Value {
        let Some(v) = &self.available else { return Value::Null };
        let percent = self.total.filter(|t| *t > 0).map(|t| (self.done * 100 / t).min(100));
        json!({
            "version": v,
            "dismissed": self.dismissed,
            "install": can_install,
            "step": self.step,
            "percent": percent,
            "error": self.error,
        })
    }
}

fn parse(v: &str) -> Option<Version> {
    let mut parts = v.strip_prefix('v').unwrap_or(v).split('.');
    let mut num = || parts.next().filter(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))?.parse().ok();
    let version = (num()?, num()?, num()?);
    parts.next().is_none().then_some(version)
}

async fn latest(b: &Arc<Bridge>) -> Result<Version, String> {
    let url = format!("{}{LATEST_FILE}", b.env.origin);
    let mut res = b
        .client()
        .get(&url)
        .header(CACHE_CONTROL, "no-cache")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            return Err("the file is too big".into());
        }
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    v.get("version").and_then(Value::as_str).and_then(parse).ok_or_else(|| "no version in it".into())
}

pub(crate) fn watch_updates(b: &Arc<Bridge>) {
    // 1.2.3-beta counts as 1.2.3
    let Some(current) = VERSION.split(['-', '+']).next().and_then(parse) else { return };
    if !b.env.update_check {
        return;
    }
    let b2 = b.clone();
    b.spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            match latest(&b2).await {
                Ok(v) => {
                    let newer = (v > current).then(|| format!("{}.{}.{}", v.0, v.1, v.2));
                    let changed = {
                        let mut s = b2.st();
                        let changed = s.update.available != newer;
                        if changed {
                            s.update.available = newer.clone();
                            s.update.dismissed = false;
                        }
                        changed
                    };
                    if changed {
                        if let Some(v) = newer {
                            log!("update: Hashi {v} is out");
                        }
                        b2.broadcast();
                    }
                }
                Err(e) => log!("update check: {e}"),
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

pub(crate) fn can_install(b: &Bridge) -> bool {
    b.opts.updater.as_ref().is_some_and(|u| u.can_install())
}

pub(crate) fn install(b: &Arc<Bridge>) {
    let Some(updater) = b.opts.updater.clone().filter(|u| u.can_install()) else { return };
    {
        let mut s = b.st();
        if s.update.available.is_none() || matches!(s.update.step, Some("downloading" | "installing")) {
            return;
        }
        s.update.step = Some("downloading");
        s.update.done = 0;
        s.update.total = None;
        s.update.error = None;
    }
    b.broadcast();
    let url = format!("{}{LATEST_FILE}", b.env.origin);
    let b2 = b.clone();
    let progress: Progress = Box::new(move |done, total| {
        {
            let mut s = b2.st();
            s.update.done = done;
            s.update.total = total;
            if total.is_some_and(|t| done >= t) {
                s.update.step = Some("installing");
            }
        }
        b2.broadcast();
    });
    let b2 = b.clone();
    b.spawn(async move {
        log!("update: installing");
        let result = updater.install(url, progress).await;
        let mut s = b2.st();
        match result {
            Ok(()) => s.update.step = Some("installing"),
            Err(e) => {
                log!("update failed: {e}");
                s.update.step = Some("failed");
                s.update.error = Some(e);
            }
        }
        drop(s);
        b2.broadcast();
    });
}

pub(crate) fn open_download_page(b: &Bridge) {
    b.open_browser(&format!("{}{DOWNLOAD_PAGE}", b.env.origin));
}

pub(crate) fn dismiss(b: &Arc<Bridge>) {
    b.st().update.dismissed = true;
    b.broadcast();
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn versions() {
        assert_eq!(parse("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse("v1.10.3"), Some((1, 10, 3)));
        assert!(parse("1.2").is_none());
        assert!(parse("1.2.3.4").is_none());
        assert!(parse("1.2.3-beta").is_none());
        assert!(parse("+1.2.3").is_none());
        assert!(parse("1..3").is_none());
        assert!(parse("1.2.3<script>").is_none());
        assert!(parse("99999999999.0.0").is_none());
        assert!((0, 10, 0) > (0, 9, 9));
    }
}
