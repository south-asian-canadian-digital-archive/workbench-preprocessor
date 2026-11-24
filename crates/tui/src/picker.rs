//! The system's own folder window, for people who'd rather click than type a
//! path. It runs on its own thread (the screen keeps drawing) and gives back
//! the folder, or None when the window was closed without one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PROMPT: &str = "Folder with the access files";

pub fn pick_folder(start: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let start = start.filter(|p| p.is_dir()).map(|p| p.to_string_lossy().to_string());
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("osascript");
        // The start folder goes in as an argument, so no quoting inside the script.
        let choose = if start.is_some() {
            format!("POSIX path of (choose folder with prompt \"{PROMPT}\" default location (POSIX file (item 1 of argv)))")
        } else {
            format!("POSIX path of (choose folder with prompt \"{PROMPT}\")")
        };
        c.args(["-e", "on run argv", "-e", &choose, "-e", "end run"]);
        c.args(start.iter());
        c
    } else if cfg!(windows) {
        // A top-most owner window, or the dialog can open behind the terminal.
        let script = format!(
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; \
             $o = New-Object System.Windows.Forms.Form -Property @{{TopMost = $true; ShowInTaskbar = $false}}; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = '{PROMPT}'; \
             if ($env:ORGANISE_START) {{ $d.SelectedPath = $env:ORGANISE_START }}; \
             if ($d.ShowDialog($o) -eq 'OK') {{ [Console]::Out.Write($d.SelectedPath) }}"
        );
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-STA", "-Command", &script]);
        c.env("ORGANISE_START", start.clone().unwrap_or_default());
        c
    } else {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Err("no desktop to open a folder window on (over SSH?) - type the path instead; Tab completes it".into());
        }
        let has = |p: &str| Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok();
        if has("zenity") {
            let mut c = Command::new("zenity");
            c.args(["--file-selection", "--directory", &format!("--title={PROMPT}")]);
            if let Some(s) = &start {
                c.arg(format!("--filename={s}/"));
            }
            c
        } else if has("kdialog") {
            let mut c = Command::new("kdialog");
            c.args(["--title", PROMPT, "--getexistingdirectory", start.as_deref().unwrap_or(".")]);
            c
        } else {
            return Err("no folder window available (install zenity) - type the path instead; Tab completes it".into());
        }
    };
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("couldn't open a folder window: {e} - type the path instead; Tab completes it"))?;
    let picked = String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']).to_string();
    if picked.is_empty() {
        // Closed without a folder (osascript, zenity and kdialog all exit 1 for
        // that), or the window was closed some other way without saying why.
        let err = String::from_utf8_lossy(&out.stderr);
        if out.status.success() || out.status.code() == Some(1) || err.contains("-128") || err.trim().is_empty() {
            return Ok(None);
        }
        return Err(format!("the folder window failed: {}", err.trim()));
    }
    Ok(Some(PathBuf::from(picked)))
}
