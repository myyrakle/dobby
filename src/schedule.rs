use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::util::{home, run};

const LABEL: &str = "dev.dobby.gc";

pub fn log_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Logs/dobby.log")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/state"))
            .join("dobby/dobby.log")
    }
}

fn exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        eprintln!(
            "warning: scheduling a binary inside a build directory ({}). \
             Prefer `cargo install --path .` first.",
            exe.display()
        );
    }
    Ok(exe)
}

/// 스케줄러는 로그인 셸의 PATH 를 모른다. 지금 PATH 를 쓰되, fnm/nvm 의 세션별 심볼릭 링크 같은
/// 임시 경로는 실제 경로로 풀어서 저장한다.
fn stable_path() -> String {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path)
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| seen.insert(p.clone()))
        .collect();
    std::env::join_paths(dirs)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn plist_path() -> PathBuf {
    home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

fn systemd_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("systemd/user")
}

fn uid() -> String {
    crate::util::run_ok("id", &["-u"]).unwrap_or_default()
}

pub fn install(hour: u8, minute: u8) -> Result<String> {
    if hour > 23 || minute > 59 {
        bail!("invalid time {hour:02}:{minute:02}");
    }
    let exe = exe()?;
    let log = log_path();
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let path_env = stable_path();

    if cfg!(target_os = "macos") {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>clean</string>
    <string>--auto</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>{path}</string>
  </dict>
  <key>StartCalendarInterval</key>
  <dict>
    <key>Hour</key><integer>{hour}</integer>
    <key>Minute</key><integer>{minute}</integer>
  </dict>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
            exe = xml_escape(&exe.to_string_lossy()),
            path = xml_escape(&path_env),
            log = xml_escape(&log.to_string_lossy()),
        );
        let p = plist_path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let domain = format!("gui/{}", uid());
        // 이미 등록돼 있으면 내렸다가 다시 올린다
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        std::fs::write(&p, plist).with_context(|| format!("writing {}", p.display()))?;
        let out = run("launchctl", &["bootstrap", &domain, &p.to_string_lossy()])?;
        if !out.status.success() {
            bail!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(format!(
            "installed launchd agent {} (daily {hour:02}:{minute:02}), log: {}",
            p.display(),
            log.display()
        ))
    } else {
        let dir = systemd_dir();
        std::fs::create_dir_all(&dir)?;
        let service = format!(
            "[Unit]\nDescription=dobby local GC\n\n[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\n\
             Environment=\"PATH={path_env}\"\nExecStart={exe} clean --auto\n\
             StandardOutput=append:{log}\nStandardError=append:{log}\n",
            exe = exe.display(),
            log = log.display(),
        );
        let timer = format!(
            "[Unit]\nDescription=Run dobby daily\n\n[Timer]\nOnCalendar=*-*-* {hour:02}:{minute:02}:00\n\
             Persistent=true\n\n[Install]\nWantedBy=timers.target\n"
        );
        std::fs::write(dir.join("dobby.service"), service)?;
        std::fs::write(dir.join("dobby.timer"), timer)?;
        for args in [
            &["--user", "daemon-reload"][..],
            &["--user", "enable", "--now", "dobby.timer"][..],
        ] {
            let out = run("systemctl", args)?;
            if !out.status.success() {
                bail!(
                    "systemctl {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
        }
        Ok(format!(
            "installed systemd user timer in {} (daily {hour:02}:{minute:02}), log: {}",
            dir.display(),
            log.display()
        ))
    }
}

pub fn uninstall() -> Result<String> {
    if cfg!(target_os = "macos") {
        let _ = run("launchctl", &["bootout", &format!("gui/{}/{LABEL}", uid())]);
        let p = plist_path();
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        Ok(format!("removed {}", p.display()))
    } else {
        let _ = run("systemctl", &["--user", "disable", "--now", "dobby.timer"]);
        let dir = systemd_dir();
        for f in ["dobby.service", "dobby.timer"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        Ok(format!("removed dobby units from {}", dir.display()))
    }
}

pub fn status() -> String {
    let installed = if cfg!(target_os = "macos") {
        let p = plist_path();
        let loaded = run("launchctl", &["print", &format!("gui/{}/{LABEL}", uid())])
            .map(|o| o.status.success())
            .unwrap_or(false);
        format!(
            "plist: {} ({}), loaded: {loaded}",
            p.display(),
            if p.exists() { "present" } else { "absent" }
        )
    } else {
        crate::util::run_ok(
            "systemctl",
            &["--user", "list-timers", "dobby.timer", "--no-pager"],
        )
        .unwrap_or_else(|| "dobby.timer not found".into())
    };
    format!("{installed}\nlog: {}", log_path().display())
}
