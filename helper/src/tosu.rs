use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

use crate::types::{truthy, Binding, Cover, Item, Play};
use crate::{iso_now, log, now_ms, replays, Bridge, AUTOMATED_MODS};

#[derive(Clone)]
pub(crate) struct Beatmap {
    md5: Option<String>,
    artist: Value,
    title: Value,
    version: Value,
    set: Value,
    keys: Value,
}

#[derive(Clone)]
pub(crate) struct LastPlay {
    client: String,
    mode: Value,
    // play.mode and resultsScreen.mode can say osu on a mania map, beatmap.mode doesn't
    beatmap_mode: Value,
    mods: Value,
    player_name: Value,
    // "playing", or someone else's play / a replay, which aren't sent
    whose: &'static str,
    beatmap: Beatmap,
    // lazer gives paths relative to files/, by hash
    chart_file: Option<String>,
    // relative to Songs (or files/ on lazer), for the cover in the ui
    cover: Option<Cover>,
}

pub(crate) struct PendingFinish {
    since: i64,
    play: LastPlay,
    viewed: Option<String>,
    stale: Option<String>,
}

// tosu keeps the socket open without osu!, it just stops sending
// launch: null, "starting" (we ran it, not up yet), "failed", "missing" (nowhere to be found) or "with-osu" (osu-winello runs it)
// blocked: { fix } when osu! runs but tosu can't read it (linux ptrace)
pub(crate) struct TosuState {
    pub connected: bool,
    pub osu: bool,
    pub client: Option<String>,
    pub state: Option<String>,
    pub player: Option<String>,
    pub launch: Option<&'static str>,
    pub blocked: Option<String>,
    pub found: Option<bool>,
    pub lazer_files_dir: PathBuf,
    pub last_frame_at: i64,
    pub last_frame: Option<Value>,
    prev_state: Option<String>,
    last_play: Option<LastPlay>,
    last_result_shown: Option<String>,
    viewed_result: Option<String>,
    // results of the last play finished here, a retry starts from those
    last_finished: Option<String>,
    // started from the results of some other score: stable's local scores open there first
    pub replay: bool,
    pending_finish: Option<PendingFinish>,
    connected_at: i64,
    exe: Option<PathBuf>,
    // send it where to say it's stopped
    kill: Option<oneshot::Sender<oneshot::Sender<()>>>,
}

impl TosuState {
    pub fn new(lazer_files_dir: PathBuf) -> Self {
        TosuState {
            connected: false,
            osu: false,
            client: None,
            state: None,
            player: None,
            launch: None,
            blocked: None,
            found: None,
            lazer_files_dir,
            last_frame_at: 0,
            last_frame: None,
            prev_state: None,
            last_play: None,
            last_result_shown: None,
            viewed_result: None,
            last_finished: None,
            replay: false,
            pending_finish: None,
            connected_at: 0,
            exe: None,
            kill: None,
        }
    }

    pub fn public(&self) -> Value {
        json!({
            "connected": self.connected,
            "osu": self.osu,
            "client": self.client,
            "state": self.state,
            "player": self.player,
            "launch": self.launch,
            "blocked": self.blocked.as_ref().map(|fix| json!({ "fix": fix })),
        })
    }

    fn gone(&mut self) {
        self.osu = false;
        self.client = None;
        self.state = None;
        self.player = None;
    }
}

pub(crate) fn mods_number(mods: &Value) -> i64 {
    mods.get("number").and_then(Value::as_i64).unwrap_or(0)
}

pub(crate) fn mods_name(mods: &Value) -> String {
    mods.get("name").and_then(Value::as_str).unwrap_or("").to_string()
}

fn is_mania(mode: &Value) -> bool {
    mode.get("number").and_then(Value::as_i64) == Some(3)
        || mode.get("name").and_then(Value::as_str).is_some_and(|n| n.to_lowercase().contains("mania"))
}

fn result_key(rs: &Value) -> String {
    let g = |k: &str| rs.get(k).cloned().unwrap_or(Value::Null);
    json!([g("createdAt"), g("score"), g("hits"), g("playerName")]).to_string()
}

fn populated(rs: &Value) -> bool {
    rs.get("playerName").is_some_and(|v| !v.is_null())
        || rs.get("createdAt").is_some_and(|v| !v.is_null())
        || rs.get("score").and_then(Value::as_f64).is_some_and(|s| s > 0.0)
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(String::from)
}

fn on_tosu_message(b: &Arc<Bridge>, raw: Value) {
    let mut finished = None;
    {
        let mut s = b.st();
        let account = s.config.connection.as_ref().map(|c| crate::types::text(&c.username));
        let t = &mut s.tosu;
        t.last_frame_at = now_ms();
        if !t.osu {
            t.osu = true;
            t.blocked = None;
            b.broadcast();
        }
        let state = str_of(raw.pointer("/state/name"))
            .or_else(|| raw.pointer("/state/number").map(|n| n.to_string()))
            .unwrap_or_else(|| "unknown".into());
        let player = str_of(raw.pointer("/profile/name"));
        let client = if raw.get("client").and_then(Value::as_str) == Some("lazer") { "lazer" } else { "stable" };
        if client == "lazer" {
            if let Some(songs) = raw.pointer("/folders/songs").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                t.lazer_files_dir = PathBuf::from(songs);
            }
        }
        if t.state.as_deref() != Some(&state) || t.player != player || t.client.as_deref() != Some(client) {
            t.state = Some(state.clone());
            t.player = player;
            t.client = Some(client.into());
            b.broadcast();
        }

        if state == "play" {
            if t.prev_state.as_deref() != Some("play") {
                // retry or watch replay from results, remember which score was up
                t.viewed_result = if t.prev_state.as_deref() == Some("resultScreen") { t.last_result_shown.clone() } else { None };
                t.replay = t.viewed_result.is_some() && t.viewed_result != t.last_finished;
            }
            let null = Value::Null;
            let bm = raw.get("beatmap").unwrap_or(&null);
            let g = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
            let files = raw.get("files").unwrap_or(&null);
            let background = str_of(files.get("background")).filter(|x| !x.is_empty());
            t.last_play = Some(LastPlay {
                client: client.into(),
                mode: raw.pointer("/play/mode").cloned().unwrap_or(Value::Null),
                beatmap_mode: g(bm, "mode"),
                mods: raw.pointer("/play/mods").cloned().unwrap_or(Value::Null),
                player_name: raw.pointer("/play/playerName").cloned().unwrap_or(Value::Null),
                whose: match raw.pointer("/play/playerName").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                    _ if t.replay => "watching_replay",
                    Some(who) => crate::presence::whose_play(&raw, who, account.as_deref()),
                    None => "playing",
                },
                beatmap: Beatmap {
                    md5: str_of(bm.get("checksum")).map(|c| c.to_lowercase()),
                    artist: g(bm, "artist"),
                    title: g(bm, "title"),
                    version: g(bm, "version"),
                    set: g(bm, "set"),
                    keys: bm
                        .pointer("/stats/cs/converted")
                        .filter(|v| !v.is_null())
                        .or(bm.pointer("/stats/cs/original"))
                        .cloned()
                        .unwrap_or(Value::Null),
                },
                chart_file: if client == "lazer" {
                    str_of(files.get("beatmap")).filter(|x| !x.is_empty()).map(|f| t.lazer_files_dir.join(f).to_string_lossy().into_owned())
                } else {
                    None
                },
                cover: if client == "lazer" {
                    background.map(|file| Cover { lazer: Some(true), folder: None, file })
                } else {
                    match (str_of(raw.pointer("/folders/beatmap")).filter(|x| !x.is_empty()), background) {
                        (Some(folder), Some(file)) => Some(Cover { lazer: None, folder: Some(folder), file }),
                        _ => None,
                    }
                },
            });
        }

        if t.prev_state.as_deref() == Some("play") && state == "resultScreen" {
            if let Some(play) = t.last_play.clone() {
                t.pending_finish = Some(PendingFinish { since: now_ms(), play, viewed: t.viewed_result.clone(), stale: t.last_result_shown.clone() });
            }
        }

        if let Some(pending) = &t.pending_finish {
            // resultsScreen lags a few polls behind the state change
            let rs = raw.get("resultsScreen").cloned().unwrap_or(json!({}));
            let fresh = state == "resultScreen" && populated(&rs) && Some(result_key(&rs)) != pending.stale;
            if state != "resultScreen" || fresh || now_ms() - pending.since > 5000 {
                let finish = t.pending_finish.take().unwrap();
                if fresh && finish.play.whose == "playing" {
                    t.last_finished = Some(result_key(&rs));
                }
                finished = Some((finish, fresh.then_some(rs)));
            }
        }

        if state == "resultScreen" {
            if let Some(rs) = raw.get("resultsScreen").filter(|rs| populated(rs)) {
                t.last_result_shown = Some(result_key(rs));
            }
        }

        t.prev_state = Some(state);
        t.last_frame = Some(raw);
    }
    if let Some((finish, rs)) = finished {
        on_play_finished(b, finish, rs);
    }
}

fn on_play_finished(b: &Arc<Bridge>, finish: PendingFinish, rs: Option<Value>) {
    let PendingFinish { play, viewed, stale, .. } = finish;
    let bm = &play.beatmap;
    let label = format!("{} - {} [{}]", crate::types::text(&bm.artist), crate::types::text(&bm.title), crate::types::text(&bm.version));
    if play.whose != "playing" {
        return log!("skip ({}, {}): {label}", play.whose.replace('_', " "), crate::types::text(&play.player_name));
    }
    // same results as before = watched the replay
    if rs.is_none() && viewed.is_some() && viewed == stale {
        return log!("skip (watched replay): {label}");
    }
    let rs = rs.unwrap_or(json!({}));
    let g = |k: &str| rs.get(k).cloned().unwrap_or(Value::Null);
    // the .osr header decides for real before anything is sent
    if ![g("mode"), play.mode.clone(), play.beatmap_mode.clone()].iter().any(is_mania) {
        return log!("skip (not mania): {label}");
    }
    let mods = rs.get("mods").filter(|m| !m.is_null()).cloned().unwrap_or(play.mods.clone());
    if mods_number(&mods) & AUTOMATED_MODS != 0 {
        return log!("skip (automated mods {}): {label}", mods_name(&mods));
    }
    let Some(md5) = bm.md5.clone() else {
        return log!("skip (tosu gave no beatmap checksum): {label}");
    };
    let created = g("createdAt");

    let item = {
        let mut s = b.st();
        if s.sends_no_plays() {
            drop(s);
            return log!("skip (sending plays is off): {label}");
        }
        // watch replay right after playing
        if truthy(&created) && s.queue.items.iter().any(|i| i.play.beatmap_md5 == md5 && i.play.result_created_at == created) {
            drop(s);
            return log!("skip (results already captured): {label}");
        }
        let status = s.connection.status.clone();
        let binding = s.config.connection.as_ref().filter(|c| c.refresh_token.is_some() && ["connected", "offline", "not_in_beta"].contains(&status.as_str())).map(|c| Binding {
            issuer: c.issuer.clone(),
            environment: c.environment.clone(),
            user_id: c.user_id.clone(),
            installation_id: c.installation_id.clone(),
        });
        let or = |v: Value, w: Value| if v.is_null() { w } else { v };
        let item = Item {
            id: format!("cbt-{}", uuid::Uuid::new_v4()),
            session_id: b.session_id.clone(),
            captured_at: iso_now(),
            detected_at_ms: now_ms(),
            binding,
            play: Play {
                client: Some(play.client.clone()),
                beatmap_md5: md5,
                artist: bm.artist.clone(),
                title: bm.title.clone(),
                version: bm.version.clone(),
                set: bm.set.clone(),
                keys: bm.keys.clone(),
                cover: play.cover.clone(),
                chart_file: play.chart_file.clone(),
                mods: mods_name(&mods),
                player_name: or(g("playerName"), play.player_name.clone()),
                score: g("score"),
                accuracy: g("accuracy"),
                result_created_at: created,
            },
            phase: "waiting_replay".into(),
            replay: None,
            chart: None,
            submission_id: None,
            receipt: None,
            chart_sent: false,
            error: None,
            attempts: 0,
            next_attempt_at: 0,
            chart_needed: None,
            poll_delay: None,
            held_from: None,
            on_hold: false,
        };
        s.queue.items.push(item.clone());
        item
    };
    b.save_queue_later();
    log!("finished: {label} {}; waiting for the replay file", if item.play.mods.is_empty() { "NM" } else { &item.play.mods });
    b.broadcast();
    let b2 = b.clone();
    b.spawn(async move { replays::match_replays(&b2).await });
}

pub(crate) fn connect_tosu(b: &Arc<Bridge>) {
    let b2 = b.clone();
    b.spawn(async move {
        let b = b2;
        let url = format!("ws://{}/websocket/v2", b.env.tosu_host);
        let mut warned = false;
        loop {
            if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(url.as_str()).await {
                {
                    let mut s = b.st();
                    s.tosu.connected = true;
                    s.tosu.connected_at = now_ms();
                    if s.tosu.launch == Some("starting") {
                        s.tosu.launch = None;
                    }
                }
                warned = false;
                log!("tosu connected");
                b.broadcast();
                while let Some(Ok(msg)) = ws.next().await {
                    let parsed = match msg {
                        Message::Text(t) => serde_json::from_str::<Value>(t.as_str()),
                        Message::Binary(bytes) => serde_json::from_slice::<Value>(&bytes),
                        Message::Close(_) => break,
                        _ => continue,
                    };
                    match parsed {
                        Ok(raw) => on_tosu_message(&b, raw),
                        Err(e) => log!("tosu frame error: {e}"),
                    }
                }
            }
            {
                let mut s = b.st();
                if s.tosu.connected || !warned {
                    log!("tosu not reachable; retrying every 2s");
                }
                warned = true;
                let t = &mut s.tosu;
                t.connected = false;
                t.gone();
                t.prev_state = None;
                t.last_play = None;
                t.pending_finish = None;
            }
            b.broadcast();
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

pub(crate) fn watch_osu_frames(b: &Arc<Bridge>) {
    let b2 = b.clone();
    b.spawn(async move {
        let mut second = tokio::time::interval(Duration::from_secs(1));
        let mut n = 0u32;
        loop {
            second.tick().await;
            let stopped = {
                let mut s = b2.st();
                let stopped = s.tosu.osu && now_ms() - s.tosu.last_frame_at > 3000;
                if stopped {
                    s.tosu.gone();
                }
                stopped
            };
            if stopped {
                b2.broadcast();
            }
            n += 1;
            // walks all of /proc on linux
            if n % 15 == 0 {
                let b3 = b2.clone();
                tokio::task::spawn_blocking(move || check_tosu_blocked(&b3));
            }
        }
    });
}

// pid of the first process with this name, linux only
fn find_process(names: &[&str]) -> Option<String> {
    let dir = std::fs::read_dir("/proc").ok()?;
    for entry in dir.flatten() {
        let pid = entry.file_name().to_string_lossy().into_owned();
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
            if names.contains(&comm.trim()) {
                return Some(pid);
            }
        }
    }
    None
}

// yama's ptrace_scope 1+ (most distros) stops tosu reading osu!'s memory unless it has cap_sys_ptrace.
// tosu then answers with nothing, which looks exactly like osu! being closed
fn check_tosu_blocked(b: &Arc<Bridge>) {
    let (connected, osu, since, exe) = {
        let s = b.st();
        (s.tosu.connected, s.tosu.osu, s.tosu.connected_at, s.tosu.exe.clone())
    };
    let mut blocked = None;
    // a fresh tosu takes a few seconds to find the game
    if cfg!(not(windows)) && connected && !osu && now_ms() - since > 10_000 {
        let scope: i64 = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        if scope >= 1 && find_process(&["osu!", "osu!.exe"]).is_some() {
            let exe = exe.or_else(|| find_process(&["tosu"]).and_then(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).ok()));
            let shown = exe.map(|e| shell_quote(&e.display().to_string())).unwrap_or("path/to/tosu".into());
            blocked = Some(format!("sudo setcap cap_sys_ptrace=eip {shown}"));
        }
    }
    let changed = {
        let mut s = b.st();
        let changed = s.tosu.blocked != blocked;
        if changed {
            if let Some(fix) = &blocked {
                log!("osu! is running but tosu reads nothing; fix: {fix}");
            }
            s.tosu.blocked = blocked;
        }
        changed
    };
    if changed {
        b.broadcast();
    }
}

// for a command that gets copied into a terminal
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn subdirs(parent: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(parent) else { return vec![] };
    dir.flatten()
        .filter(|d| d.file_type().is_ok_and(|t| t.is_dir()) && d.file_name().to_string_lossy().to_lowercase().starts_with(prefix))
        .map(|d| d.path())
        .collect()
}

fn find_tosu(b: &Bridge) -> Option<PathBuf> {
    let home = crate::home();
    let mut candidates: Vec<PathBuf> = b.env.tosu_path.iter().cloned().collect();
    candidates.push(crate::get_tosu::install_dir(b).join(crate::get_tosu::exe_name()));
    if cfg!(windows) {
        let v = |k: &str, d: PathBuf| crate::var(k).map(PathBuf::from).unwrap_or(d);
        let local = v("LOCALAPPDATA", home.join("AppData").join("Local"));
        let roaming = v("APPDATA", home.join("AppData").join("Roaming"));
        let programs = v("ProgramFiles", PathBuf::from("C:\\Program Files"));
        let mut dirs: Vec<PathBuf> =
            [local, roaming, programs, home.clone(), b.env.osu_dir.clone(), PathBuf::from("C:\\")].iter().map(|d| d.join("tosu")).collect();
        // e.g. Downloads\tosu-windows-v4.3.1
        for parent in ["Downloads", "Desktop", "OneDrive\\Desktop", "Documents", ""] {
            dirs.extend(subdirs(&home.join(parent), "tosu"));
        }
        candidates.extend(dirs.iter().map(|d| d.join("tosu.exe")));
    } else {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        dirs.extend([home.join(".local").join("bin"), home.join("tosu"), home.join(".local").join("share").join("tosu")]);
        dirs.extend(subdirs(&home.join("Applications"), "tosu"));
        candidates.extend(dirs.iter().map(|d| d.join("tosu")));
    }
    candidates.into_iter().find(|f| f.is_file())
}

fn autostart_on(b: &Bridge) -> bool {
    b.env.tosu_can_start && b.st().config.settings.as_ref().is_some_and(|x| x.tosu_autostart)
}

pub(crate) fn check_tosu_launch(b: &Arc<Bridge>) {
    if !b.env.tosu_can_start {
        return;
    }
    let b2 = b.clone();
    b.spawn(async move {
        let b = b2;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let found = find_tosu(&b).is_some();
        b.st().tosu.found = Some(found);
        if b.st().tosu.connected {
            return b.broadcast();
        }
        // no native tosu, but osu-winello's launcher runs its wine one next to stable
        if cfg!(not(windows)) && !found && b.env.osu_dir.join("launch_with_memory.bat").exists() {
            log!("tosu starts with osu! (osu-winello), not starting it here");
            b.st().tosu.launch = Some("with-osu");
            b.broadcast();
            return;
        }
        if autostart_on(&b) {
            start_tosu(&b);
        }
        b.broadcast();
    });
}

pub(crate) fn recheck_found(b: &Arc<Bridge>) {
    if !b.env.tosu_can_start || b.st().tosu.connected || b.st().get_tosu.step.is_some() {
        return;
    }
    let found = Some(find_tosu(b).is_some());
    let changed = {
        let mut s = b.st();
        let changed = s.tosu.found != found;
        s.tosu.found = found;
        if found == Some(true) && s.tosu.launch == Some("missing") {
            s.tosu.launch = None;
        }
        changed
    };
    if changed {
        b.broadcast();
    }
}

pub(crate) async fn set_tosu_autostart(b: &Arc<Bridge>, on: bool) -> bool {
    if !b.env.tosu_can_start {
        return true;
    }
    let start = {
        let mut s = b.st();
        s.config.settings.get_or_insert_with(Default::default).tosu_autostart = on;
        if !on && matches!(s.tosu.launch, Some("missing" | "failed")) {
            s.tosu.launch = None;
        }
        on && !s.tosu.connected && s.tosu.kill.is_none() && s.tosu.launch != Some("with-osu")
    };
    let saved = b.save_config().await;
    log!("starting tosu with the bridge {}", if on { "on" } else { "off" });
    if start {
        start_tosu(b);
    }
    b.broadcast();
    saved
}

fn start_tosu(b: &Arc<Bridge>) {
    let Some(exe) = find_tosu(b) else {
        log!("tosu not found, start it yourself or set TOSU_PATH");
        b.st().tosu.launch = Some("missing");
        return b.broadcast();
    };
    log!("starting tosu: {}", exe.display());
    // tosu keeps its config next to the exe
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.current_dir(exe.parent().unwrap_or(Path::new(".")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let (kill_tx, kill_rx) = oneshot::channel();
    {
        let mut s = b.st();
        s.tosu.exe = Some(exe.clone());
        s.tosu.launch = Some("starting");
        s.tosu.kill = Some(kill_tx);
    }
    b.broadcast();
    let b2 = b.clone();
    b.spawn(async move {
        let b = b2;
        let failed = |why: String| {
            let mut s = b.st();
            s.tosu.kill = None;
            if s.tosu.connected {
                return;
            }
            log!("tosu didn't start: {why}");
            s.tosu.launch = Some("failed");
            drop(s);
            b.broadcast();
        };
        match cmd.spawn() {
            Err(e) => failed(e.to_string()),
            Ok(mut child) => {
                tokio::select! {
                    status = child.wait() => {
                        let code = status.ok().and_then(|s| s.code()).map_or("null".into(), |c| c.to_string());
                        failed(format!("exited with {code}"));
                    }
                    stopped = kill_rx => {
                        let _ = child.kill().await;
                        if let Ok(stopped) = stopped {
                            let _ = stopped.send(());
                        }
                    }
                }
            }
        }
    });
}

// only the one we started, a tosu the player opened stays up
// waits for it, the process is about to exit and kill_on_drop doesn't run then
pub(crate) async fn stop_tosu(b: &Arc<Bridge>) {
    let kill = b.st().tosu.kill.take();
    let Some(kill) = kill else { return };
    let (stopped_tx, stopped) = oneshot::channel();
    if kill.send(stopped_tx).is_ok() {
        let _ = tokio::time::timeout(Duration::from_millis(500), stopped).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quoting() {
        assert_eq!(super::shell_quote("/home/a/.local/bin/tosu"), "/home/a/.local/bin/tosu");
        assert_eq!(super::shell_quote("/home/a/My Apps/tosu"), "'/home/a/My Apps/tosu'");
        assert_eq!(super::shell_quote("/x/it's/$tosu"), r"'/x/it'\''s/$tosu'");
    }
}
