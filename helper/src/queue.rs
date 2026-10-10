use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};

use crate::auth::{self, api, Fail, Req, Resp};
use crate::types::{Blob, Item, ItemError};
use crate::{log, now_ms, replays, Bridge};

const TERMINAL_STATES: [&str; 4] = ["accepted", "rejected", "expired", "deleted"];

#[derive(Default)]
pub(crate) struct PumpState {
    // 429
    paused_until: i64,
    pumping: bool,
    // woken while pumping, look once more before stopping
    again: bool,
}

fn title(item: &Item) -> String {
    crate::types::text(&item.play.title)
}

pub(crate) fn fail(b: &Arc<Bridge>, id: &str, phase: &str, code: &str, message: &str) {
    if let Some(item) = b.st().queue.get_mut(id) {
        item.phase = phase.into();
        item.error = ItemError::new(code, message);
        log!("{} [{}]: {phase} ({code}) {message}", title(item), crate::types::text(&item.play.version));
    }
    b.save_queue_later();
    b.broadcast();
}

// before it's completed on the site a play can still be taken back
const UNSENT: [&str; 5] = ["waiting_replay", "send", "held", "failed", "unbound"];

pub(crate) fn remove(b: &Arc<Bridge>, id: &str) -> bool {
    {
        let mut s = b.st();
        let before = s.queue.items.len();
        s.queue.items.retain(|i| !(i.id == id && UNSENT.contains(&i.phase.as_str())));
        if s.queue.items.len() == before {
            return false;
        }
    }
    log!("removed a play that wasn't sent");
    b.save_queue_later();
    b.broadcast();
    true
}

pub(crate) fn hold(b: &Arc<Bridge>, id: &str, on: bool) -> bool {
    {
        let mut s = b.st();
        let Some(item) = s.queue.get_mut(id).filter(|i| ["waiting_replay", "send"].contains(&i.phase.as_str())) else { return false };
        item.on_hold = on;
        log!("{}: {}", title(item), if on { "on hold" } else { "sending after all" });
    }
    b.save_queue_later();
    b.broadcast();
    if !on {
        wake_queue(b, 0);
    }
    true
}

fn retry_delay(attempts: u32) -> i64 {
    const STEPS: [i64; 6] = [5_000, 15_000, 60_000, 5 * 60_000, 15 * 60_000, 30 * 60_000];
    STEPS[(attempts as usize).min(STEPS.len() - 1)]
}

fn retry_later(item: &mut Item, message: &str, delay_ms: Option<i64>) {
    item.attempts += 1;
    let wait = delay_ms.unwrap_or_else(|| retry_delay(item.attempts - 1));
    item.next_attempt_at = now_ms() + wait;
    item.error = ItemError::new("retrying", message);
    log!("{}: retry in {}s ({message})", title(item), (wait as f64 / 1000.0).round());
}

pub(crate) fn wake_queue(b: &Arc<Bridge>, delay_ms: i64) {
    let gen = b.wake_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let b2 = b.clone();
    b.spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms.max(0) as u64)).await;
        if b2.wake_gen.load(Ordering::SeqCst) == gen {
            pump(&b2).await;
        }
    });
}

fn binding_matches(s: &crate::State, item: &Item) -> bool {
    match (&s.config.connection, &item.binding) {
        (Some(c), Some(bd)) => {
            bd.issuer == c.issuer && bd.environment == c.environment && bd.installation_id == c.installation_id && bd.user_id == c.user_id
        }
        _ => false,
    }
}

fn same_account(s: &crate::State, item: &Item) -> bool {
    match (&s.config.connection, &item.binding) {
        (Some(c), Some(bd)) => !bd.user_id.is_null() && bd.issuer == c.issuer && bd.environment == c.environment && bd.user_id == c.user_id,
        _ => false,
    }
}

fn next_due(b: &Arc<Bridge>) -> Option<Item> {
    let mut s = b.st();
    if s.connection.status != "connected" {
        return None;
    }
    let now = now_ms();
    if now < s.pump.paused_until {
        wake_queue(b, (s.pump.paused_until - now).min(60_000));
        return None;
    }
    let mut changed = false;
    let mut active: Vec<usize> = vec![];
    let sending = !s.sends_no_plays();
    for idx in 0..s.queue.items.len() {
        let i = &s.queue.items[idx];
        if !["send", "poll", "held"].contains(&i.phase.as_str()) || ((i.on_hold || !sending) && i.phase == "send") {
            continue;
        }
        let mut matches = binding_matches(&s, &s.queue.items[idx]);
        if !matches && same_account(&s, &s.queue.items[idx]) {
            let c = s.config.connection.as_ref().expect("same_account has a connection");
            let installation_id = c.installation_id.clone();
            let i = &mut s.queue.items[idx];
            if let Some(bd) = i.binding.as_mut() {
                bd.installation_id = installation_id;
            }
            log!("{}: from an earlier connection to the same account, going on under this one", title(i));
            matches = true;
            changed = true;
        }
        let i = &mut s.queue.items[idx];
        let account_changed = i.error.as_ref().is_some_and(|e| e.code == "account_changed");
        if !matches {
            if !account_changed {
                i.held_from = Some(i.phase.clone());
                i.phase = "held".into();
                i.error = ItemError::new("account_changed", "Played while connected to another account. It wasn't sent.");
                changed = true;
            }
            continue;
        }
        if account_changed {
            i.phase = i.held_from.take().unwrap_or("send".into());
            i.error = None;
            i.next_attempt_at = 0;
            changed = true;
        }
        active.push(idx);
    }
    if changed {
        b.save_queue_later();
        b.broadcast();
    }
    let due = active.iter().filter(|&&i| s.queue.items[i].next_attempt_at <= now).min_by_key(|&&i| s.queue.items[i].next_attempt_at);
    match due {
        Some(&i) => Some(s.queue.items[i].clone()),
        None => {
            if let Some(next) = active.iter().map(|&i| s.queue.items[i].next_attempt_at).min() {
                // the 60s tick covers the rest
                wake_queue(b, (next - now).clamp(250, 60_000));
            }
            None
        }
    }
}

pub(crate) async fn pump(b: &Arc<Bridge>) {
    {
        let mut s = b.st();
        if s.pump.pumping {
            s.pump.again = true;
            return;
        }
        s.pump.pumping = true;
        s.pump.again = false;
    }
    loop {
        pump_due(b).await;
        let mut s = b.st();
        if !std::mem::take(&mut s.pump.again) {
            s.pump.pumping = false;
            return;
        }
    }
}

async fn pump_due(b: &Arc<Bridge>) {
    while let Some(mut item) = next_due(b) {
        let result = step(b, &mut item).await;
        let problem = match result {
            Ok(()) => None,
            Err(Fail::Conn(status, message)) => Some((status, message)),
            Err(Fail::Other(message)) => {
                retry_later(&mut item, &message, None);
                None
            }
        };
        {
            let mut s = b.st();
            if item.phase == "done" {
                s.boards.forget(&item.play.beatmap_md5);
            }
            if let Some(slot) = s.queue.get_mut(&item.id) {
                // held while it was being sent: that holds until it's completed
                item.on_hold = slot.on_hold && item.phase == "send";
                *slot = item;
            }
        }
        if let Some((status, message)) = problem {
            auth::set_connection(b, status, Some(message));
            break;
        }
        b.save_queue().await;
        b.broadcast();
    }
}

fn state_of(receipt: &Value) -> &str {
    receipt.get("state").and_then(Value::as_str).unwrap_or("")
}

fn new_poll(item: &mut Item) {
    item.phase = "poll".into();
    item.poll_delay = Some(2000);
    item.next_attempt_at = now_ms() + 1000;
}

// reserving again is idempotent, so always start from the current receipt
async fn step(b: &Arc<Bridge>, item: &mut Item) -> Result<(), Fail> {
    if item.phase == "held" {
        item.phase = "send".into();
    }
    if item.phase == "poll" {
        return poll(b, item).await;
    }

    let Some(mut receipt) = reserve(b, item).await? else { return Ok(()) };
    for _ in 0..3 {
        if TERMINAL_STATES.contains(&state_of(&receipt)) {
            settle(item, receipt);
            return Ok(());
        }
        if state_of(&receipt) != "awaiting_assets" {
            new_poll(item);
            return Ok(());
        }
        if crate::types::truthy(receipt.get("needs_replay").unwrap_or(&Value::Null)) {
            let path = receipt.get("replay_upload_path").cloned().unwrap_or(Value::Null);
            let blob = item.replay.clone();
            let Some(next) = upload(b, item, &path, blob).await? else { return Ok(()) };
            receipt = next;
        }
        if crate::types::truthy(receipt.get("needs_beatmap").unwrap_or(&Value::Null)) {
            let path = receipt.get("beatmap_upload_path").cloned().unwrap_or(Value::Null);
            let blob = item.chart.clone();
            if upload(b, item, &path, blob).await?.is_none() {
                return Ok(());
            }
            item.chart_sent = true;
        }
        // removed, held or sending turned off while the files went up: completing is what makes it count
        if !still_wanted(b, &item.id) {
            return Ok(());
        }
        let id = item.submission_id.clone().unwrap_or_default();
        let res = api(b, &format!("/submissions/{id}/complete"), Req::method(reqwest::Method::POST)).await?;
        let body = res.json();
        if res.status == 202 || res.ok() {
            log!("{}: completed, {}", title(item), state_of(&body));
            item.receipt = Some(body);
            new_poll(item);
            item.error = None;
            item.attempts = 0;
            return Ok(());
        }
        let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
        if res.status == 409 && ["assets_missing", "wrong_state"].contains(&code) {
            // server lost a file, re-read and resend
            let Some(next) = read_receipt(b, item).await? else { return Ok(()) };
            receipt = next;
            continue;
        }
        handle_error(b, item, &res, &body, "complete")?;
        return Ok(());
    }
    retry_later(item, "The server kept asking for the files again.", None);
    Ok(())
}

fn still_wanted(b: &Arc<Bridge>, id: &str) -> bool {
    let s = b.st();
    !s.sends_no_plays() && s.queue.items.iter().any(|i| i.id == id && !i.on_hold)
}

fn manifest(item: &Item) -> Value {
    let replay = item.replay.as_ref();
    let chart = item.chart.as_ref();
    let mut m = json!({
        "protocol_version": 1,
        "client_version": auth::client_version(),
        "game_client": item.play.client.as_deref().unwrap_or("stable"),
        "capture_kind": "automatic_completed_play",
        "replay": { "sha256": replay.map(|r| r.sha256.clone()), "byte_length": replay.map(|r| r.byte_length) },
        "chart": { "md5": chart.and_then(|c| c.md5.clone()), "sha256": chart.map(|c| c.sha256.clone()), "byte_length": chart.map(|c| c.byte_length) },
    });
    if let Some(at) = replay.and_then(|r| r.played_at.clone()) {
        m["observed"] = json!({ "finished_at": at });
    }
    m
}

async fn reserve(b: &Arc<Bridge>, item: &mut Item) -> Result<Option<Value>, Fail> {
    let mut req = Req::json(reqwest::Method::POST, &manifest(item));
    req.headers.push(("idempotency-key", item.id.clone()));
    let res = api(b, "/submissions", req).await?;
    let body = res.json();
    if !res.ok() {
        return handle_error(b, item, &res, &body, "reserve");
    }
    let id = body.get("submission_id").and_then(Value::as_str).map(String::from);
    if item.submission_id.is_none() {
        log!(
            "{}: reserved {} (needs replay {}, chart {})",
            title(item),
            id.as_deref().unwrap_or("?"),
            body.get("needs_replay").unwrap_or(&Value::Null),
            body.get("needs_beatmap").unwrap_or(&Value::Null)
        );
    }
    item.submission_id = id;
    if item.chart_needed.is_none() {
        item.chart_needed = Some(body.get("needs_beatmap") == Some(&Value::Bool(true)));
    }
    item.receipt = Some(body.clone());
    Ok(Some(body))
}

async fn read_receipt(b: &Arc<Bridge>, item: &mut Item) -> Result<Option<Value>, Fail> {
    let id = item.submission_id.clone().unwrap_or_default();
    let res = api(b, &format!("/submissions/{id}"), Req::get()).await?;
    let body = res.json();
    if !res.ok() {
        return handle_error(b, item, &res, &body, "receipt");
    }
    item.receipt = Some(body.clone());
    Ok(Some(body))
}

// only paths the server gave us, same origin
async fn upload_url(b: &Arc<Bridge>, p: &Value) -> Result<String, Fail> {
    let caps = auth::caps(b).await?;
    let prefix = format!("{}/submissions/", caps.get("api_prefix").and_then(Value::as_str).unwrap_or(""));
    let refuse = || Fail::Other(format!("Refusing upload path {}", crate::types::text(p)));
    let p = p.as_str().filter(|p| p.starts_with(&prefix)).ok_or_else(refuse)?;
    let issuer = url::Url::parse(caps.get("issuer").and_then(Value::as_str).unwrap_or("")).map_err(|_| refuse())?;
    let url = issuer.join(p).map_err(|_| refuse())?;
    if url.origin() != issuer.origin() {
        return Err(refuse());
    }
    Ok(url.to_string())
}

async fn upload(b: &Arc<Bridge>, item: &mut Item, path: &Value, blob: Option<Blob>) -> Result<Option<Value>, Fail> {
    let blob = blob.ok_or_else(|| Fail::Other("Nothing stored to upload.".into()))?;
    let bytes = replays::read_blob(b, &blob).await.map_err(Fail::Other)?;
    let url = upload_url(b, path).await?;
    let req = Req {
        method: reqwest::Method::PUT,
        headers: vec![("content-type", "application/octet-stream".into())],
        body: Some(Bytes::from(bytes)),
        // server cuts uploads at 4 min
        timeout: Duration::from_secs(5 * 60),
    };
    let res = api(b, &url, req).await?;
    let body = res.json();
    if !res.ok() {
        return handle_error(b, item, &res, &body, "upload");
    }
    item.receipt = Some(body.clone());
    Ok(Some(body))
}

async fn poll(b: &Arc<Bridge>, item: &mut Item) -> Result<(), Fail> {
    let Some(receipt) = read_receipt(b, item).await? else { return Ok(()) };
    let state = state_of(&receipt).to_string();
    if TERMINAL_STATES.contains(&state.as_str()) {
        settle(item, receipt);
        return Ok(());
    }
    item.error = None;
    if state == "awaiting_assets" {
        item.phase = "send".into();
        item.next_attempt_at = now_ms();
        return Ok(());
    }
    // deferred retries are 1-16 min apart
    let cap = if state == "deferred" { 60_000 } else { 30_000 };
    let delay = ((item.poll_delay.unwrap_or(2000) as f64 * 1.6).round() as i64).min(cap);
    item.poll_delay = Some(delay);
    item.next_attempt_at = now_ms() + delay;
    Ok(())
}

fn settle(item: &mut Item, receipt: Value) {
    let error = receipt.get("error").filter(|e| !e.is_null());
    item.error = error.map(|e| ItemError {
        code: e.get("code").and_then(Value::as_str).unwrap_or("").into(),
        message: e.get("message").and_then(Value::as_str).map(String::from),
    });
    let code = error.and_then(|e| e.get("code")).and_then(Value::as_str).map(|c| format!(" ({c})")).unwrap_or_default();
    log!("{}: {}{code}", title(item), state_of(&receipt));
    item.receipt = Some(receipt);
    item.phase = "done".into();
    item.next_attempt_at = 0;
}

fn handle_error(b: &Arc<Bridge>, item: &mut Item, res: &Resp, body: &Value, stage: &str) -> Result<Option<Value>, Fail> {
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .map(String::from)
        .unwrap_or_else(|| format!("http_{}", res.status));
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| body.get("error_description").and_then(Value::as_str))
        .map(String::from)
        .unwrap_or_else(|| format!("{stage} answered {}", res.status));
    let retry_after = res.header("retry-after").and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
    let retry_after_ms = retry_after.map(|s| (s * 1000.0) as i64);

    if res.status == 401 {
        // token_expired already got its one refresh in api()
        if code == "token_expired" {
            b.st().access_token = None;
            retry_later(item, "The access token expired again right after a refresh.", None);
            return Ok(None);
        }
        if code == "installation_revoked" {
            return Err(Fail::Conn("revoked", "This connection was removed on Mania Tracker. Connect again.".into()));
        }
        if ["token_revoked", "invalid_token", "wrong_environment"].contains(&code.as_str()) {
            return Err(Fail::Conn("revoked", auth::GONE.into()));
        }
        // key_mismatch, invalid_dpop_proof, htu_mismatch, ...
        return Err(Fail::Conn("error", format!("The server refused this client's proof ({code}). Uploads are stopped.")));
    }
    if res.status == 403 && code == "account_not_allowed" {
        return Err(Fail::Conn("not_in_beta", message));
    }
    if res.status == 404 {
        item.phase = "done".into();
        item.error = ItemError::new("gone", "The server no longer has this submission.");
        return Ok(None);
    }
    if res.status == 429 {
        let wait = retry_after_ms.unwrap_or(60_000);
        b.st().pump.paused_until = now_ms() + wait;
        retry_later(item, &message, Some(wait));
        return Ok(None);
    }
    if res.status == 409 && code == "chart_environment_conflict" {
        item.phase = "held".into();
        item.error = ItemError::new(&code, message);
        item.next_attempt_at = now_ms() + 30 * 60_000;
        return Ok(None);
    }
    if res.status == 504 && stage == "upload" {
        retry_later(item, &message, Some(10 * 60_000));
        return Ok(None);
    }
    if body.pointer("/error/retryable") == Some(&Value::Bool(true)) || res.status >= 500 || body.get("error").and_then(Value::as_str) == Some("server_error") {
        retry_later(item, &message, retry_after_ms);
        return Ok(None);
    }
    // digest_mismatch, not_mania, payload_too_large etc, permanent
    item.phase = "failed".into();
    let shown = if code == "unsupported_game_client" { "Mania Tracker doesn't take lazer plays from the app yet.".to_string() } else { message.clone() };
    item.error = ItemError::new(&code, shown);
    log!("{}: refused at {stage} ({} {code}) {message}", title(item), res.status);
    Ok(None)
}

fn play_url(r: &Value, origin: &str, username: Option<&str>) -> Option<String> {
    let state = r.get("state").and_then(Value::as_str)?;
    let duplicate = r.pointer("/error/code").and_then(Value::as_str) == Some("duplicate_play");
    if state != "accepted" && !(state == "rejected" && duplicate) {
        return None;
    }
    let encode = |s| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC);
    let list = format!("{origin}/player/{}/recent", encode(username?));
    match r.get("local_score_id").and_then(Value::as_str).filter(|id| state == "accepted" && !id.is_empty()) {
        Some(id) => Some(format!("{list}?import={}", encode(id))),
        None => Some(list),
    }
}

fn rating(r: &Value) -> Value {
    let Some(x) = r.get("rating").filter(|_| state_of(r) == "accepted") else { return Value::Null };
    let msd = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|m| m.is_finite() && (0.0..=100.0).contains(m));
    let dan = x.get("dan").and_then(|d| {
        let label = d.get("label").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty() && l.chars().count() <= 32)?;
        let ln = d.get("family").and_then(Value::as_str) == Some("ln");
        Some(json!({ "label": label, "ln": ln, "eligible": d.get("eligible").and_then(Value::as_bool).unwrap_or(true) }))
    });
    let msd = msd(x.get("msd"));
    if msd.is_none() && dan.is_none() {
        return Value::Null;
    }
    json!({ "msd": msd, "dan": dan })
}

pub(crate) fn public_item(item: &Item, origin: &str, username: Option<&str>) -> Value {
    let r = item.receipt.as_ref();
    json!({
        "id": item.id,
        "capturedAt": item.captured_at,
        "play": {
            "client": item.play.client.as_deref().unwrap_or("stable"),
            "artist": item.play.artist,
            "title": item.play.title,
            "version": item.play.version,
            "keys": item.play.keys,
            "mods": item.play.mods,
            "score": item.play.score,
            "accuracy": item.play.accuracy,
        },
        "phase": item.phase,
        "onHold": item.on_hold,
        "retryAt": if item.error.as_ref().is_some_and(|e| e.code == "retrying") { Some(item.next_attempt_at) } else { None },
        "receipt": r.map(|r| json!({
            "state": r.get("state"),
            "submissionId": r.get("submission_id"),
            "error": r.get("error").cloned().unwrap_or(Value::Null),
        })),
        "cover": item.play.cover.is_some() || (item.chart.as_ref().is_some_and(|c| c.source.is_some()) && !item.play.is_lazer()),
        "chartNeeded": item.chart_needed,
        "chartSent": item.chart_sent,
        "error": item.error,
        "url": r.and_then(|r| play_url(r, origin, username)),
        "rating": r.map(rating).unwrap_or(Value::Null),
        "md5": item.play.beatmap_md5,
        "rank": r.filter(|r| state_of(r) == "accepted").map(crate::leaderboard::rank).unwrap_or(Value::Null),
    })
}

// keep finished plays 30 days. true if any went
pub(crate) async fn prune_queue(b: &Arc<Bridge>) -> bool {
    let cutoff = crate::iso_from_ms(now_ms() - 30 * 24 * 3600_000);
    let (keep, pruned): (std::collections::HashSet<String>, bool) = {
        let mut s = b.st();
        let before = s.queue.items.len();
        s.queue.items.retain(|i| !(["done", "failed", "unbound"].contains(&i.phase.as_str()) && i.captured_at < cutoff));
        let pruned = s.queue.items.len() != before;
        let mut keep: std::collections::HashSet<String> =
            s.queue.items.iter().flat_map(|i| [i.replay.as_ref(), i.chart.as_ref()]).flatten().map(|x| x.sha256.clone()).collect();
        for raw in &s.queue.unparsed {
            for k in ["/replay/sha256", "/chart/sha256"] {
                if let Some(h) = raw.pointer(k).and_then(Value::as_str) {
                    keep.insert(h.into());
                }
            }
        }
        (keep, pruned)
    };
    let Ok(mut dir) = tokio::fs::read_dir(&b.env.blob_dir).await else { return pruned };
    while let Ok(Some(e)) = dir.next_entry().await {
        if keep.contains(e.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        // a replay being matched right now is stored before its play points at it
        let old = e.metadata().await.and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d > Duration::from_secs(3600)));
        if old {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
    pruned
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn play_links() {
        let o = "https://mania-tracker.com";
        let url = |r, name| super::play_url(&r, o, name);
        assert_eq!(url(json!({ "state": "accepted" }), Some("Aleju03")).as_deref(), Some("https://mania-tracker.com/player/Aleju03/recent"));
        assert_eq!(url(json!({ "state": "accepted" }), Some("a b")).as_deref(), Some("https://mania-tracker.com/player/a%20b/recent"));
        assert_eq!(
            url(json!({ "state": "accepted", "local_score_id": "kf9PXHG_HhFN3HvwyX2lAQ" }), Some("Aleju03")).as_deref(),
            Some("https://mania-tracker.com/player/Aleju03/recent?import=kf9PXHG%5FHhFN3HvwyX2lAQ")
        );
        assert_eq!(url(json!({ "state": "rejected", "local_score_id": "x1", "error": { "code": "duplicate_play" } }), Some("x")).as_deref(), Some("https://mania-tracker.com/player/x/recent"));
        assert_eq!(url(json!({ "state": "rejected", "error": { "code": "duplicate_play" } }), Some("x")).as_deref(), Some("https://mania-tracker.com/player/x/recent"));
        assert_eq!(url(json!({ "state": "rejected", "error": { "code": "bad_replay" } }), Some("x")), None);
        assert_eq!(url(json!({ "state": "queued" }), Some("x")), None);
        assert_eq!(url(json!({ "state": "accepted" }), None), None);
    }

    #[test]
    fn ratings() {
        let rating = |r| super::rating(&r);
        let dan = json!({ "label": "8+", "family": "dan", "raw": 8.21, "eligible": false });
        assert_eq!(
            rating(json!({ "state": "accepted", "rating": { "msd": 24.31, "dan": dan } })),
            json!({ "msd": 24.31, "dan": { "label": "8+", "ln": false, "eligible": false } })
        );
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "msd": 24.31, "dan": null } })), json!({ "msd": 24.31, "dan": null }));
        // unrated: the chart's msd alone isn't the play's
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "analysis_state": "unsupported", "msd": null, "chart_msd": 25.02, "dan": null } })), json!(null));
        assert_eq!(
            rating(json!({ "state": "accepted", "rating": { "msd": 30.5, "dan": { "label": "12", "family": "ln", "eligible": true } } })),
            json!({ "msd": 30.5, "dan": { "label": "12", "ln": true, "eligible": true } })
        );
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "msd": null, "dan": null } })), json!(null));
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "msd": 1e9, "dan": { "label": " " } } })), json!(null));
        assert_eq!(rating(json!({ "state": "accepted" })), json!(null));
        assert_eq!(rating(json!({ "state": "queued", "rating": { "msd": 24.31 } })), json!(null));
    }
}
