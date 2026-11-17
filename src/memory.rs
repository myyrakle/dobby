use serde::Serialize;

use crate::util::{command, run_ok};

#[derive(Debug, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub rss_bytes: u64,
    pub command: String,
}

#[derive(Debug, Serialize)]
pub struct MemoryReport {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub top: Vec<Proc>,
}

#[derive(Debug, Serialize)]
pub struct PurgeResult {
    pub ok: bool,
    pub message: String,
}

pub fn report(top_n: usize) -> MemoryReport {
    let (total, avail) = totals();
    MemoryReport {
        total_bytes: total,
        available_bytes: avail,
        top: top_processes(top_n),
    }
}

#[cfg(target_os = "macos")]
fn totals() -> (Option<u64>, Option<u64>) {
    let total = run_ok("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse().ok());
    // vm_stat: 페이지 단위. free + inactive + purgeable + speculative 를 가용으로 본다.
    let avail = run_ok("vm_stat", &[]).and_then(|text| {
        let page: u64 = text
            .lines()
            .next()?
            .split("page size of ")
            .nth(1)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        let get = |key: &str| -> u64 {
            text.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
                .unwrap_or(0)
        };
        Some(
            (get("Pages free")
                + get("Pages inactive")
                + get("Pages purgeable")
                + get("Pages speculative"))
                * page,
        )
    });
    (total, avail)
}

#[cfg(not(target_os = "macos"))]
fn totals() -> (Option<u64>, Option<u64>) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let get = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    (get("MemTotal:"), get("MemAvailable:"))
}

fn top_processes(n: usize) -> Vec<Proc> {
    let Some(text) = run_ok("ps", &["-axo", "pid=,rss=,comm="]) else {
        return Vec::new();
    };
    let mut procs: Vec<Proc> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let rss_kb: u64 = it.next()?.parse().ok()?;
            let command = it.collect::<Vec<_>>().join(" ");
            Some(Proc {
                pid,
                rss_bytes: rss_kb * 1024,
                command,
            })
        })
        .collect();
    procs.sort_by_key(|p| std::cmp::Reverse(p.rss_bytes));
    procs.truncate(n);
    procs
}

/// 파일 시스템 캐시를 비운다. root 권한이 필요하며, 비밀번호를 묻지 않도록 `sudo -n` 으로 시도한다.
pub fn purge() -> PurgeResult {
    let args: &[&str] = if cfg!(target_os = "macos") {
        &["-n", "/usr/sbin/purge"]
    } else {
        &[
            "-n",
            "sh",
            "-c",
            "sync && echo 3 > /proc/sys/vm/drop_caches",
        ]
    };
    match command("sudo", args).output() {
        Ok(o) if o.status.success() => PurgeResult {
            ok: true,
            message: "purged file system caches".into(),
        },
        Ok(o) => PurgeResult {
            ok: false,
            message: format!(
                "purge needs passwordless sudo for this command ({}). stderr: {}",
                args[1..].join(" "),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        },
        Err(e) => PurgeResult {
            ok: false,
            message: format!("failed to run sudo: {e}"),
        },
    }
}
