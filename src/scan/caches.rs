use std::path::PathBuf;

use crate::config::Config;
use crate::model::{Action, Candidate, Category, Safety};
use crate::util::{has_cmd, home, run_ok};

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn user_cache_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Caches")
    } else {
        env_path("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache"))
    }
}

fn run_action(program: &str, args: &[&str]) -> Action {
    Action::Run {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

struct Spec {
    kind: &'static str,
    paths: Vec<PathBuf>,
    action: Action,
    safety: Safety,
    reason: &'static str,
    /// prune 계열이라 실제로 비워지는 양은 캐시 크기보다 작다.
    estimate: bool,
}

/// 원칙: 쓰지 않는 것만 지우는 prune 계열은 Safe, 캐시 전체를 지우는 것은 Review.
fn specs() -> Vec<Spec> {
    let mut v = Vec::new();
    let cache = user_cache_dir();

    let cargo_home = env_path("CARGO_HOME").unwrap_or_else(|| home().join(".cargo"));
    v.push(Spec {
        kind: "cargo-registry",
        paths: vec![
            cargo_home.join("registry/cache"),
            cargo_home.join("registry/src"),
            cargo_home.join("git/checkouts"),
        ],
        action: Action::RemoveDirs {
            paths: vec![
                cargo_home.join("registry/cache"),
                cargo_home.join("registry/src"),
                cargo_home.join("git/checkouts"),
            ],
        },
        safety: Safety::Review,
        reason: "downloaded crates; re-downloaded on next build",
        estimate: false,
    });

    if has_cmd("go") {
        if let Some(p) = run_ok("go", &["env", "GOCACHE"]) {
            v.push(Spec {
                kind: "go-build",
                paths: vec![PathBuf::from(p)],
                action: run_action("go", &["clean", "-cache"]),
                safety: Safety::Safe,
                reason: "go build cache; rebuilt locally",
                estimate: false,
            });
        }
        if let Some(p) = run_ok("go", &["env", "GOMODCACHE"]) {
            v.push(Spec {
                kind: "go-mod",
                paths: vec![PathBuf::from(p)],
                action: run_action("go", &["clean", "-modcache"]),
                safety: Safety::Review,
                reason: "go module cache; re-downloaded on next build",
                estimate: false,
            });
        }
    }

    if has_cmd("npm") {
        v.push(Spec {
            kind: "npm",
            paths: vec![home().join(".npm/_cacache")],
            action: run_action("npm", &["cache", "clean", "--force"]),
            safety: Safety::Review,
            reason: "npm download cache",
            estimate: false,
        });
    }
    if has_cmd("pnpm")
        && let Some(p) = run_ok("pnpm", &["store", "path"])
    {
        v.push(Spec {
            kind: "pnpm-store",
            paths: vec![PathBuf::from(p)],
            action: run_action("pnpm", &["store", "prune"]),
            safety: Safety::Safe,
            reason: "removes packages no project references",
            estimate: true,
        });
    }
    if has_cmd("yarn") {
        let p = if cfg!(target_os = "macos") {
            cache.join("Yarn")
        } else {
            cache.join("yarn")
        };
        v.push(Spec {
            kind: "yarn",
            paths: vec![p, home().join(".yarn/berry/cache")],
            action: run_action("yarn", &["cache", "clean"]),
            safety: Safety::Review,
            reason: "yarn download cache",
            estimate: false,
        });
    }
    if has_cmd("bun") {
        v.push(Spec {
            kind: "bun",
            paths: vec![home().join(".bun/install/cache")],
            action: run_action("bun", &["pm", "cache", "rm"]),
            safety: Safety::Review,
            reason: "bun install cache",
            estimate: false,
        });
    }

    if has_cmd("uv")
        && let Some(p) = run_ok("uv", &["cache", "dir"])
    {
        v.push(Spec {
            kind: "uv",
            paths: vec![PathBuf::from(p)],
            action: run_action("uv", &["cache", "prune"]),
            safety: Safety::Safe,
            reason: "removes unused uv cache entries",
            estimate: true,
        });
    }
    let pip_dir = cache.join("pip");
    if pip_dir.exists() {
        v.push(Spec {
            kind: "pip",
            paths: vec![pip_dir.clone()],
            action: Action::RemoveDirs {
                paths: vec![pip_dir],
            },
            safety: Safety::Review,
            reason: "pip download/wheel cache",
            estimate: false,
        });
    }

    if has_cmd("brew")
        && let Some(p) = run_ok("brew", &["--cache"])
    {
        v.push(Spec {
            kind: "homebrew",
            paths: vec![PathBuf::from(p)],
            action: run_action("brew", &["cleanup", "--prune=30"]),
            safety: Safety::Safe,
            reason: "old formula versions and downloads older than 30 days",
            estimate: true,
        });
    }

    let gradle = home().join(".gradle/caches");
    v.push(Spec {
        kind: "gradle",
        paths: vec![gradle.clone()],
        action: Action::RemoveDirs {
            paths: vec![gradle],
        },
        safety: Safety::Review,
        reason: "gradle dependency/build cache",
        estimate: false,
    });
    let m2 = home().join(".m2/repository");
    v.push(Spec {
        kind: "maven",
        paths: vec![m2.clone()],
        action: Action::RemoveDirs { paths: vec![m2] },
        safety: Safety::Review,
        reason: "maven local repository",
        estimate: false,
    });

    if cfg!(target_os = "macos") {
        let dd = home().join("Library/Developer/Xcode/DerivedData");
        v.push(Spec {
            kind: "xcode-derived-data",
            paths: vec![dd.clone()],
            action: Action::RemoveDirs { paths: vec![dd] },
            safety: Safety::Safe,
            reason: "Xcode build intermediates",
            estimate: false,
        });
    }
    v
}

pub fn scan(cfg: &Config) -> Vec<Candidate> {
    specs()
        .into_iter()
        .filter(|s| !cfg.caches.disabled.iter().any(|d| d == s.kind))
        .filter_map(|s| {
            let existing: Vec<PathBuf> = s.paths.into_iter().filter(|p| p.exists()).collect();
            let first = existing.first()?.clone();
            let mut c = Candidate::new(
                Category::Cache,
                s.kind,
                format!("cache:{}", s.kind),
                s.safety,
                s.action,
            )
            .with_path(first)
            .sized_by(existing)
            .reason(s.reason);
            if s.estimate {
                c = c.estimate();
            }
            Some(c)
        })
        .collect()
}
