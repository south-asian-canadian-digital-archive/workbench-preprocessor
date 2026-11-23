//! Copying text to the clipboard from a terminal program: the system's own
//! tool when there is one, and the OSC 52 escape as well (it reaches the
//! clipboard of the machine the terminal runs on, also over SSH).

use std::io::Write;
use std::process::{Command, Stdio};

/// Copy `text`; says how it was done.
pub fn copy(text: &str) -> Result<&'static str, String> {
    let tools: &[(&str, &[&str], &'static str)] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[], "pbcopy")]
    } else if cfg!(windows) {
        &[("powershell", &["-NoProfile", "-Command", "$input | Set-Clipboard"], "Set-Clipboard"), ("clip", &[], "clip")]
    } else {
        &[
            ("wl-copy", &[], "wl-copy"),
            ("xclip", &["-selection", "clipboard"], "xclip"),
            ("xsel", &["--clipboard", "--input"], "xsel"),
        ]
    };
    // Wayland's tool only when it's a Wayland session, X11's only with a display.
    let usable = |name: &str| match name {
        "wl-copy" => std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "xclip" | "xsel" => std::env::var_os("DISPLAY").is_some(),
        _ => true,
    };
    for (cmd, args, how) in tools {
        if usable(cmd) && pipe_to(cmd, args, text) {
            return Ok(how);
        }
    }
    osc52(text).map(|_| "terminal clipboard (OSC 52)").map_err(|e| e.to_string())
}

fn pipe_to(cmd: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(cmd).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    let wrote = child.stdin.take().map(|mut i| i.write_all(text.as_bytes()).is_ok()).unwrap_or(false);
    wrote && child.wait().map(|s| s.success()).unwrap_or(false)
}

fn osc52(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            s.push(if i <= c.len() { T[(n >> shift & 63) as usize] as char } else { '=' });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
