mod artifacts;
mod caches;
mod docker;
mod worktrees;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::model::{Candidate, Category, Safety};
use crate::util::{home, par_map};

pub use artifacts::Rule;

/// 파일 시스템을 한 번 훑어서 찾은 것들.
#[derive(Default)]
pub struct Found {
    /// (산출물 디렉토리, 규칙)
    pub artifacts: Vec<(PathBuf, &'static Rule)>,
    /// 메인 git 저장소 경로 (worktree 가 아닌 쪽).
    pub repos: BTreeSet<PathBuf>,
}

/// 내려가지 않는 디렉토리 이름.
const NEVER_DESCEND: &[&str] = &[
    ".git",
    "node_modules",
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
];

/// 이 파일이 있는 디렉토리 아래는 건너뛴다.
pub const IGNORE_FILE: &str = ".dobbyignore";

pub fn scan(cfg: &Config, cats: &[Category]) -> Vec<Candidate> {
    let wants = |c: Category| cats.is_empty() || cats.contains(&c);
    let mut out = Vec::new();

    if (wants(Category::Artifact) && cfg.artifacts.enabled)
        || (wants(Category::Worktree) && cfg.worktrees.enabled)
    {
        let found = walk_roots(cfg);
        if wants(Category::Worktree) && cfg.worktrees.enabled {
            out.extend(worktrees::scan(cfg, &found.repos));
        }
        if wants(Category::Artifact) && cfg.artifacts.enabled {
            out.extend(artifacts::scan(cfg, &found.artifacts));
        }
    }
    if wants(Category::Cache) && cfg.caches.enabled {
        out.extend(caches::scan(cfg));
    }
    if wants(Category::Docker) && cfg.docker.enabled {
        out.extend(docker::scan(cfg));
    }

    let mut out = measure(out);
    let min_artifact = cfg.artifacts.min_size_mb * 1024 * 1024;
    out.retain(|c| {
        c.category != Category::Artifact
            || c.safety == Safety::Blocked
            || c.bytes.unwrap_or(0) >= min_artifact
    });
    out.retain(|c| c.category == Category::Worktree || c.bytes != Some(0));
    out.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then(b.bytes.unwrap_or(0).cmp(&a.bytes.unwrap_or(0)))
    });
    out
}

fn measure(cands: Vec<Candidate>) -> Vec<Candidate> {
    par_map(cands, |mut c| {
        if !c.size_paths.is_empty() {
            c.bytes = Some(c.size_paths.iter().map(|p| crate::util::dir_size(p)).sum());
        }
        c
    })
}

pub fn walk_roots(cfg: &Config) -> Found {
    let mut found = Found::default();
    let mut seen = BTreeSet::new();
    for root in cfg.root_paths() {
        if let Ok(root) = root.canonicalize()
            && seen.insert(root.clone())
        {
            walk(cfg, &root, 0, &mut found);
        }
    }
    found
}

fn walk(cfg: &Config, dir: &Path, depth: usize, found: &mut Found) {
    if cfg.is_excluded(dir) || dir.join(IGNORE_FILE).exists() {
        return;
    }
    let git = dir.join(".git");
    if git.is_dir() {
        found.repos.insert(dir.to_path_buf());
    } else if git.is_file()
        && let Some(repo) = main_repo_of_worktree(&git)
    {
        found.repos.insert(repo);
    }
    if depth >= cfg.max_depth {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() {
            continue; // 심볼릭 링크 포함
        }
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = e.path();
        if let Some(rule) = artifacts::match_rule(dir, name, &path) {
            found.artifacts.push((path, rule));
            continue;
        }
        if NEVER_DESCEND.contains(&name) || name.starts_with('.') {
            continue;
        }
        if dir == home() && name == "go" {
            continue; // GOPATH 의 모듈 캐시는 cache 범주에서 다룬다
        }
        walk(cfg, &path, depth + 1, found);
    }
}

/// worktree 의 `.git` 파일(`gitdir: ...`)에서 메인 저장소 경로를 찾는다.
fn main_repo_of_worktree(git_file: &Path) -> Option<PathBuf> {
    let gitdir = git_dir_from_file(git_file)?;
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = gitdir.join(common.trim()).canonicalize().ok()?;
    // 일반 저장소면 <repo>/.git, bare 저장소면 그 자체
    if common.file_name()? == ".git" {
        Some(common.parent()?.to_path_buf())
    } else {
        Some(common)
    }
}

fn git_dir_from_file(git_file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_file).ok()?;
    let raw = text.trim().strip_prefix("gitdir:")?.trim();
    let p = PathBuf::from(raw);
    Some(if p.is_absolute() {
        p
    } else {
        git_file.parent()?.join(p)
    })
}

/// `dir` 이 속한 git 작업 트리의 index 파일. 활동 시각 판단에 쓴다.
pub fn git_index_of(dir: &Path) -> Option<PathBuf> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let git = d.join(".git");
        if git.is_dir() {
            return Some(git.join("index"));
        }
        if git.is_file() {
            return git_dir_from_file(&git).map(|g| g.join("index"));
        }
        cur = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("dobby-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn walk_finds_artifacts_and_respects_ignore() {
        let root = tmpdir("walk");
        fs::create_dir_all(root.join("rs/target/debug")).unwrap();
        fs::write(root.join("rs/Cargo.toml"), "").unwrap();
        fs::create_dir_all(root.join("js/node_modules/x")).unwrap();
        fs::write(root.join("js/package.json"), "{}").unwrap();
        // 매니페스트 없는 target 은 산출물이 아니다
        fs::create_dir_all(root.join("other/target")).unwrap();
        fs::create_dir_all(root.join("ignored/target")).unwrap();
        fs::write(root.join("ignored/Cargo.toml"), "").unwrap();
        fs::write(root.join("ignored").join(IGNORE_FILE), "").unwrap();

        let cfg = Config {
            roots: vec![root.to_string_lossy().into()],
            ..Config::default()
        };
        let found = walk_roots(&cfg);
        let mut kinds: Vec<_> = found
            .artifacts
            .iter()
            .map(|(p, r)| (p.strip_prefix(&root).unwrap().to_path_buf(), r.kind))
            .collect();
        kinds.sort();
        assert_eq!(
            kinds,
            vec![
                (PathBuf::from("js/node_modules"), "node_modules"),
                (PathBuf::from("rs/target"), "rust-target"),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
