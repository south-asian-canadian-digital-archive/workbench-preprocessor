//! `organise doctor`: is this machine ready to ingest, and can it reach each site?

use super::config::{find_g_root, user_config_path, Config};
use super::site::{Login, Site, JSONAPI_HELP};
use std::process::Command;

pub struct Line {
    pub ok: Option<bool>, // None = just information
    pub text: String,
}

fn line(ok: Option<bool>, text: impl Into<String>) -> Line {
    Line { ok, text: text.into() }
}

pub fn run(only_site: Option<&str>) -> (Vec<Line>, bool) {
    let mut out = vec![line(None, format!("organise {}", env!("CARGO_PKG_VERSION")))];
    let mut all_ok = true;

    let Some(g) = find_g_root() else {
        out.push(line(
            Some(false),
            "g/ folder not found - run from inside the workbench checkout, set SACDA_G, or re-run the installer",
        ));
        return (out, false);
    };
    out.push(line(Some(true), format!("g/ folder: {}", g.display())));
    if let Some(p) = user_config_path() {
        out.push(line(None, format!("settings file: {}{}", p.display(), if p.is_file() { "" } else { " (not written yet)" })));
    }
    let cfg = match Config::load_from(&g) {
        Ok(c) => {
            out.push(line(Some(true), "config/sacda.toml and config/schema.toml read"));
            c
        }
        Err(e) => {
            out.push(line(Some(false), format!("{e:#}")));
            return (out, false);
        }
    };

    let wb = cfg.workbench_root();
    let has_wb = wb.join("workbench").is_file();
    all_ok &= has_wb;
    out.push(line(Some(has_wb), format!("Islandora Workbench at {}", wb.display())));
    match Command::new("uv").arg("--version").output() {
        Ok(o) if o.status.success() => out.push(line(Some(true), String::from_utf8_lossy(&o.stdout).trim().to_string())),
        _ => {
            all_ok = false;
            out.push(line(Some(false), "uv isn't on PATH - the installer/update script installs it"));
        }
    }

    for (name, s) in &cfg.sacda.sites {
        if only_site.is_some_and(|o| o != name) {
            continue;
        }
        let cred = cfg.g_root.join(&s.credentials);
        if !cred.is_file() {
            out.push(line(None, format!("{name}: no {} - skipped (ask the SACDA admin if you need {name})", s.credentials)));
            continue;
        }
        let site = match Site::new(&cfg, Some(name)) {
            Ok(x) => x,
            Err(e) => {
                out.push(line(Some(false), format!("{name}: {e:#}")));
                all_ok = false;
                continue;
            }
        };
        let is_default = *name == cfg.sacda.default_site || only_site == Some(name.as_str());
        if let Err(e) = site.reachable() {
            // Only the site you'd ingest into by default decides "ready".
            out.push(line(
                if is_default { Some(false) } else { None },
                format!("{name}: {e:#}{}", if is_default { "" } else { " (not the default site - only matters if you use it)" }),
            ));
            all_ok &= !is_default;
            continue;
        }
        let api = site.jsonapi_enabled().unwrap_or(false);
        let login = site.login();
        let ok = api && login == Login::Ok;
        all_ok &= ok;
        let mut msg = format!("{name}: {} reachable", s.host);
        msg.push_str(if api { ", JSON:API on" } else { ", JSON:API OFF" });
        msg.push_str(match login {
            Login::Ok => ", signed in",
            Login::BadPassword => ", login REFUSED (check the credentials file)",
            Login::Locked => ", login 403 (flood lock? see the precheck note)",
            Login::NoCredentials(_) => ", no credentials",
        });
        out.push(line(Some(ok), msg));
        if !api {
            out.push(line(None, format!("    {JSONAPI_HELP}")));
        }
    }
    (out, all_ok)
}

pub fn print(only_site: Option<&str>) -> bool {
    let (lines, ok) = run(only_site);
    for l in lines {
        let mark = match l.ok {
            Some(true) => "  ok ",
            Some(false) => "  !! ",
            None => "     ",
        };
        println!("{mark}{}", l.text);
    }
    println!("{}", if ok { "\nReady." } else { "\nNot ready - fix the !! lines above." });
    ok
}
