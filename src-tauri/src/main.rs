#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod autostart;

use mania_bridge::{log, Bridge, Options, Progress, Updater, WindowHandler};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::UpdaterExt;

const MAIN: &str = "main";

// the window is destroyed in the tray rather than hidden, an idle web view is most of the memory
static IN_TRAY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Place {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    maximized: bool,
}

static PLACE: Mutex<Option<Place>> = Mutex::new(None);

fn place(w: &tauri::WebviewWindow) -> Option<Place> {
    let scale = w.scale_factor().ok()?;
    let at = w.outer_position().ok()?.to_logical::<f64>(scale);
    let size = w.inner_size().ok()?.to_logical::<f64>(scale);
    Some(Place { x: at.x, y: at.y, width: size.width, height: size.height, maximized: w.is_maximized().unwrap_or(false) })
}

fn build_window(app: &AppHandle, opening: bool) -> tauri::Result<()> {
    let bridge = app.state::<Arc<Bridge>>();
    let mut url = bridge.env.counter_url();
    if opening {
        url.push_str("?opening");
    }
    let url = url.parse().expect("counter url");
    let mut window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(url))
        .title("Mania Bridge")
        .inner_size(480.0, 640.0)
        .min_inner_size(360.0, 400.0)
        .decorations(!cfg!(windows))
        // the theme's --osu-b5, no white flash before the page paints
        .background_color(backdrop(bridge.theme()))
        // links that would open a new window (target=_blank) go to the browser instead
        .on_new_window(|url, _| {
            if matches!(url.scheme(), "http" | "https") {
                mania_bridge::open_url(url.as_str());
            }
            tauri::webview::NewWindowResponse::Deny
        });
    if let Some(p) = *PLACE.lock().unwrap_or_else(|e| e.into_inner()) {
        window = window.position(p.x, p.y).inner_size(p.width, p.height).maximized(p.maximized);
    }
    window.build()?;
    Ok(())
}

fn show(app: &AppHandle) {
    IN_TRAY.store(false, Ordering::SeqCst);
    match app.get_webview_window(MAIN) {
        Some(w) => {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
        None => {
            if let Err(e) = build_window(app, false) {
                log!("could not open the window: {e}");
            }
        }
    }
}

fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        if let Some(p) = place(&w) {
            *PLACE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
        }
        IN_TRAY.store(true, Ordering::SeqCst);
        let _ = w.destroy();
        // glibc holds on to what the web view freed, hand it back once it's torn down
        #[cfg(target_os = "linux")]
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(3));
            // SAFETY: malloc_trim only releases free memory, it's safe from any thread
            unsafe { libc::malloc_trim(0) };
        });
    }
}

fn toggle(app: &AppHandle) {
    match app.get_webview_window(MAIN).map(|w| w.is_visible().unwrap_or(true)) {
        Some(true) => hide(app),
        _ => show(app),
    }
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "show", "Show", true, None::<&str>)?,
            &MenuItem::with_id(app, "hide", "Hide to tray", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("tray")
        .tooltip("Mania Bridge")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "show" => show(app),
            "hide" => hide(app),
            "quit" => app.exit(0),
            _ => {}
        })
        // windows and mac, linux trays only have the menu
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                toggle(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

// --osu-b5 of the theme: hsl(hue, 10% * sat, 15% - 8% * (1 - sat)), as rgb
fn backdrop((hue, sat): (u16, u8)) -> tauri::window::Color {
    let f = f32::from(sat) / 100.0;
    let (h, s, l) = (f32::from(hue) / 360.0, 0.1 * f, 0.15 - 0.08 * (1.0 - f));
    let a = s * l.min(1.0 - l);
    let channel = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        let v = l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (v * 255.0).round() as u8
    };
    tauri::window::Color(channel(0.0), channel(8.0), channel(4.0), 255)
}

fn window_handler(app: &AppHandle) -> WindowHandler {
    let app = app.clone();
    Arc::new(move |name: &str| {
        let Some(w) = app.get_webview_window(MAIN) else { return };
        match name {
            "hide" => hide(&app),
            "minimize" => {
                let _ = w.minimize();
            }
            "close" => app.exit(0),
            "drag" => {
                let _ = w.start_dragging();
            }
            _ => {}
        }
    })
}

fn shut_down(app: &AppHandle) {
    let Some(bridge) = app.try_state::<Arc<Bridge>>().map(|b| b.inner().clone()) else { return };
    tauri::async_runtime::block_on(async move {
        let _ = tokio::time::timeout(Duration::from_secs(2), bridge.shutdown()).await;
    });
}

static RELAUNCH: AtomicBool = AtomicBool::new(false);

// runs from RunEvent::Exit, after the single-instance plugin has let go, so the new copy
// doesn't find this one still there. the helper's port it waits for by itself
fn relaunch(app: &AppHandle) {
    let env = app.env();
    let Ok(exe) = tauri::process::current_binary(&env) else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(env.args_os.iter().skip(1));
    // an appimage puts its own folders in front of PATH, LD_LIBRARY_PATH and so on. the new
    // copy brings its own, these would load the old copy's libraries and keep it mounted
    #[cfg(target_os = "linux")]
    if let Some(appdir) = std::env::var("APPDIR").ok().filter(|d| !d.is_empty()) {
        let inside = |p: &str| p == appdir || p.starts_with(&format!("{appdir}/"));
        cmd.env_clear();
        for (k, v) in std::env::vars_os() {
            match v.to_str() {
                Some(v) if v.contains(&appdir) => {
                    let kept: Vec<&str> = v.split(':').filter(|p| !p.is_empty() && !inside(p)).collect();
                    if !kept.is_empty() {
                        cmd.env(&k, kept.join(":"));
                    }
                }
                _ => {
                    cmd.env(&k, &v);
                }
            }
        }
        cmd.current_dir(std::env::var("OWD").ok().filter(|d| !inside(d)).unwrap_or_else(|| "/".into()));
        // and doesn't take along the handle this copy got on its mount, which would keep
        // the old one around for as long as the new one runs
        use std::os::unix::process::CommandExt;
        // close-on-exec rather than closed, std's own pipe for exec errors has to stay.
        // SAFETY: only close_range(2)/fcntl(2) between fork and exec, both async-signal-safe
        unsafe {
            cmd.pre_exec(|| {
                const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC) != 0 {
                    for fd in 3..4096 {
                        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    }
                }
                Ok(())
            });
        }
    }
    if let Err(e) = cmd.spawn() {
        log!("could not start the new version: {e}");
    }
}

struct AppUpdater(AppHandle);

impl Updater for AppUpdater {
    // dev builds and copies run outside an installer can't replace themselves
    fn can_install(&self) -> bool {
        tauri::utils::platform::bundle_type().is_some()
    }

    fn install(&self, url: String, progress: Progress) -> futures_util::future::BoxFuture<'static, Result<(), String>> {
        let app = self.0.clone();
        Box::pin(async move {
            let url = url.parse().map_err(|e| format!("{e}"))?;
            let shutting = app.clone();
            let updater = app
                .updater_builder()
                .endpoints(vec![url])
                .map_err(|e| e.to_string())?
                // windows: the installer closes the app itself, this runs first
                .on_before_exit(move || shut_down(&shutting))
                .build()
                .map_err(|e| e.to_string())?;
            let update = updater.check().await.map_err(|e| e.to_string())?.ok_or("There's no update for this copy.")?;
            let mut done = 0u64;
            let bytes = update
                .download(
                    |chunk, total| {
                        done += chunk as u64;
                        progress(done, total);
                    },
                    || {},
                )
                .await
                .map_err(|e| e.to_string())?;
            // .deb and .rpm ask for the password here
            tauri::async_runtime::spawn_blocking(move || update.install(bytes))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            // linux: the new copy is in place, RunEvent::Exit cleans up and starts it
            RELAUNCH.store(true, Ordering::SeqCst);
            app.exit(0);
            Ok(())
        })
    }
}

fn main() {
    // webkitgtk's dma-buf renderer dies on nvidia under wayland (gdk "Error 71 (Protocol error)"),
    // set before gtk starts. everyone else keeps it, it's the faster one
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() && std::path::Path::new("/proc/driver/nvidia").exists() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            // linux needs (ayatana-)appindicator for a tray, go on without one if it isn't there
            let has_tray = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tray(&handle))).is_ok_and(|r| r.is_ok());

            let opts = Options {
                tray: has_tray,
                titlebar: cfg!(windows),
                on_window: Some(window_handler(&handle)),
                log_file: true,
                updater: Some(Arc::new(AppUpdater(handle.clone()))),
                autostart: Some(Arc::new(autostart::LoginItem)),
            };
            autostart::refresh();
            let hidden = has_tray && std::env::args().any(|a| a == autostart::HIDDEN);
            let bridge = match tauri::async_runtime::block_on(mania_bridge::start(opts)) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            };
            if !has_tray {
                log!("no tray on this system");
            }

            app.manage(bridge);
            if hidden {
                IN_TRAY.store(true, Ordering::SeqCst);
            } else {
                build_window(&handle, true)?;
            }

            // ctrl+c, logout, pkill: quit the normal way so RunEvent::Exit still cleans up
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                #[cfg(unix)]
                let terminate = async {
                    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                        Ok(mut s) => drop(s.recv().await),
                        Err(_) => std::future::pending().await,
                    }
                };
                #[cfg(not(unix))]
                let terminate = std::future::pending::<()>();
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate => {}
                }
                handle.exit(0);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("could not start the app");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { code: None, api, .. } if IN_TRAY.load(Ordering::SeqCst) => api.prevent_exit(),
        RunEvent::Exit => {
            shut_down(app);
            if RELAUNCH.load(Ordering::SeqCst) {
                relaunch(app);
            }
        }
        _ => {}
    });
}
