use std::sync::Arc;

use serde_json::Value;

use crate::{log, Bridge};

pub trait Autostart: Send + Sync {
    fn enabled(&self) -> bool;
    fn set(&self, on: bool) -> Result<(), String>;
}

pub(crate) fn load(b: &Bridge) {
    let on = b.opts.autostart.as_ref().map(|a| a.enabled());
    b.st().autostart = on;
}

pub(crate) fn public(b: &Bridge) -> Value {
    b.st().autostart.map(Value::Bool).unwrap_or(Value::Null)
}

pub(crate) async fn set(b: &Arc<Bridge>, on: bool) -> Result<(), String> {
    let Some(a) = b.opts.autostart.clone() else { return Ok(()) };
    let result = tokio::task::spawn_blocking(move || a.set(on).map(|()| a.enabled())).await.map_err(|e| e.to_string())?;
    match result {
        Ok(now) => {
            log!("open with the computer {}", if now { "on" } else { "off" });
            b.st().autostart = Some(now);
            b.broadcast();
            Ok(())
        }
        Err(e) => {
            log!("open with the computer: {e}");
            Err(e)
        }
    }
}
