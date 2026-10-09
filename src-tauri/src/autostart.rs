// started with --hidden. on linux it points at the appimage, not the copy it mounts

use std::path::PathBuf;

use mania_bridge::Autostart;

pub const HIDDEN: &str = "--hidden";

pub struct LoginItem;

fn program() -> Result<PathBuf, String> {
    match std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        Some(p) => Ok(PathBuf::from(p)),
        None => std::env::current_exe().map_err(|e| e.to_string()),
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::PathBuf;

    fn entry() -> PathBuf {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
        config.join("autostart").join("mania-bridge.desktop")
    }

    // Exec quoting from the desktop entry spec: in double quotes with " ` $ \ escaped, then
    // the string escape doubles every backslash again. % is a field code, written %%
    fn quote(path: &str) -> String {
        let mut q = String::from("\"");
        for c in path.chars() {
            match c {
                '"' | '`' | '$' => {
                    q.push_str("\\\\");
                    q.push(c);
                }
                '\\' => q.push_str("\\\\\\\\"),
                '%' => q.push_str("%%"),
                _ => q.push(c),
            }
        }
        q.push('"');
        q
    }

    fn contents() -> Result<String, String> {
        let program = super::program()?;
        let program = program.to_str().ok_or("the app's path isn't valid text")?;
        Ok(format!(
            "[Desktop Entry]\nType=Application\nName=Hashi\nComment=Sends your osu!mania plays to Mania Tracker\nExec={} {}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            quote(program),
            super::HIDDEN
        ))
    }

    pub fn enabled() -> bool {
        entry().is_file()
    }

    pub fn set(on: bool) -> Result<(), String> {
        let file = entry();
        if !on {
            return match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, contents()?).map_err(|e| e.to_string())
    }

    // the appimage may have moved since
    pub fn refresh() {
        if let (true, Ok(want)) = (enabled(), contents()) {
            if std::fs::read_to_string(entry()).ok().as_deref() != Some(want.as_str()) {
                let _ = std::fs::write(entry(), want);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn quoting() {
            assert_eq!(super::quote("/home/a/My Apps/Hashi_0.1.0_amd64.AppImage"), "\"/home/a/My Apps/Hashi_0.1.0_amd64.AppImage\"");
            assert_eq!(super::quote("/x/100%/$y"), "\"/x/100%%/\\\\$y\"");
        }
    }
}

#[cfg(windows)]
mod imp {
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    // where task manager's startup tab keeps what it turned off
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const NAME: &str = "Hashi";

    fn command() -> Result<String, String> {
        Ok(format!("\"{}\" {}", super::program()?.display(), super::HIDDEN))
    }

    pub fn enabled() -> bool {
        windows_registry::CURRENT_USER.open(RUN).and_then(|k| k.get_string(NAME)).is_ok()
    }

    pub fn set(on: bool) -> Result<(), String> {
        let run = windows_registry::CURRENT_USER.create(RUN).map_err(|e| e.to_string())?;
        if on {
            run.set_string(NAME, command()?).map_err(|e| e.to_string())?;
            if let Ok(approved) = windows_registry::CURRENT_USER.open(APPROVED) {
                let _ = approved.remove_value(NAME);
            }
            Ok(())
        } else {
            match run.remove_value(NAME) {
                Err(_) if !enabled() => Ok(()),
                r => r.map_err(|e| e.to_string()),
            }
        }
    }

    // the install folder may have changed with a reinstall
    pub fn refresh() {
        if enabled() {
            let _ = set(true);
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn enabled() -> bool {
        false
    }
    pub fn set(_on: bool) -> Result<(), String> {
        Err("not on this system".into())
    }
    pub fn refresh() {}
}

impl Autostart for LoginItem {
    fn enabled(&self) -> bool {
        imp::enabled()
    }

    fn set(&self, on: bool) -> Result<(), String> {
        imp::set(on)
    }
}

pub fn refresh() {
    imp::refresh();
}
