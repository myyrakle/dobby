use std::collections::{HashSet, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME is not set")
}

/// `~` / `~/x` 를 홈 디렉토리 기준으로 펼친다.
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// 실제 디스크 사용량(블록 기준)을 합산한다. 심볼릭 링크는 따라가지 않고,
/// 하드링크(pnpm 등)는 한 번만 센다.
pub fn dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    let mut seen = HashSet::new();
    let mut stack = vec![p.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(md) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !md.is_dir() {
            total += md.blocks() * 512;
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            } else if md.nlink() <= 1 || seen.insert((md.dev(), md.ino())) {
                total += md.blocks() * 512;
            }
        }
    }
    total
}

pub fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(p).ok()?.modified().ok()
}

/// 디렉토리 자신과 바로 아래 항목들 중 가장 최근 수정 시각.
pub fn shallow_mtime(p: &Path) -> Option<SystemTime> {
    let mut best = mtime(p)?;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if let Some(t) = e.metadata().ok().and_then(|m| m.modified().ok()) {
                best = best.max(t);
            }
        }
    }
    Some(best)
}

pub fn age_days(t: SystemTime) -> f64 {
    SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs_f64() / 86400.0)
        .unwrap_or(0.0)
}

pub fn has_cmd(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| d.join(name).is_file())
}

pub fn command(program: &str, args: &[&str]) -> Command {
    let mut c = Command::new(program);
    c.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    c
}

pub fn run(program: &str, args: &[&str]) -> std::io::Result<Output> {
    command(program, args).output()
}

/// 성공했을 때만 stdout(trim)을 돌려준다.
pub fn run_ok(program: &str, args: &[&str]) -> Option<String> {
    let out = run(program, args).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut full = vec!["-C", dir.to_str()?];
    full.extend_from_slice(args);
    run_ok("git", &full)
}

pub fn git_status_ok(dir: &Path, args: &[&str]) -> bool {
    let Some(d) = dir.to_str() else { return false };
    let mut full = vec!["-C", d];
    full.extend_from_slice(args);
    run("git", &full)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// docker 가 출력하는 "6.949GB", "270.3kB (58%)" 같은 값을 바이트로 바꾼다.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.split_whitespace().next()?;
    let idx = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(idx);
    let n: f64 = num.parse().ok()?;
    let mul = match unit.to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((n * mul) as u64)
}

/// 아이템을 스레드 풀에서 병렬 처리한다. 결과 순서는 입력 순서와 같다.
pub fn par_map<T: Send, R: Send>(items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let n = items.len();
    let queue = Mutex::new(items.into_iter().enumerate().collect::<VecDeque<_>>());
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..n).map(|_| None).collect());
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(n.max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let Some((i, item)) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    let r = f(item);
                    results.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|r| r.expect("worker result missing"))
        .collect()
}

/// UTC 기준 "YYYY-MM-DD HH:MM:SSZ".
pub fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Howard Hinnant 의 civil_from_days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_sizes() {
        assert_eq!(parse_size("6.949GB"), Some(6_949_000_000));
        assert_eq!(parse_size("270.3kB (58%)"), Some(270_300));
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("garbage"), None);
    }

    #[test]
    fn formats_human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn par_map_keeps_order() {
        let out = par_map((0..100).collect(), |x: i32| x * 2);
        assert_eq!(out, (0..100).map(|x| x * 2).collect::<Vec<_>>());
    }
}
