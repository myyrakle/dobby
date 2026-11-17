use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::Config;
use crate::model::{Action, Candidate, Category, Safety};
use crate::util::{age_days, git, git_status_ok, has_cmd, mtime, par_map, run_ok};

#[derive(Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

/// `git worktree list --porcelain` 출력 파싱. 첫 항목이 메인 worktree 다.
pub fn parse_porcelain(text: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut wt = Worktree::default();
        for line in block.lines() {
            let (key, val) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => wt.path = PathBuf::from(val),
                "HEAD" => wt.head = val.to_string(),
                "branch" => wt.branch = Some(val.trim_start_matches("refs/heads/").to_string()),
                "locked" => wt.locked = true,
                "prunable" => wt.prunable = true,
                _ => {}
            }
        }
        if !wt.path.as_os_str().is_empty() {
            out.push(wt);
        }
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergedPr {
    number: u64,
    head_ref_name: String,
    head_ref_oid: String,
}

/// Orca 가 관리하는 worktree 경로들. Orca 에 연결할 수 없으면 None.
fn orca_paths() -> Option<HashSet<PathBuf>> {
    if !has_cmd("orca") {
        return Some(HashSet::new());
    }
    let text = run_ok("orca", &["worktree", "list", "--json"])?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let list = v.pointer("/result/worktrees")?.as_array()?;
    Some(
        list.iter()
            .filter_map(|w| w.get("path")?.as_str())
            .map(PathBuf::from)
            .collect(),
    )
}

pub fn scan(cfg: &Config, repos: &BTreeSet<PathBuf>) -> Vec<Candidate> {
    let orca = orca_paths();
    let gh = cfg.worktrees.use_gh && has_cmd("gh");
    par_map(repos.iter().cloned().collect(), |repo| {
        scan_repo(cfg, &repo, orca.as_ref(), gh)
    })
    .into_iter()
    .flatten()
    .collect()
}

fn scan_repo(
    cfg: &Config,
    repo: &Path,
    orca: Option<&HashSet<PathBuf>>,
    gh: bool,
) -> Vec<Candidate> {
    let Some(text) = git(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let wts = parse_porcelain(&text);
    if wts.len() <= 1 {
        return Vec::new();
    }
    let mut out = Vec::new();

    if wts.iter().any(|w| w.prunable) {
        out.push(
            Candidate::new(
                Category::Worktree,
                "stale-worktree-metadata",
                format!("worktree-prune:{}", repo.display()),
                Safety::Safe,
                Action::PruneWorktrees {
                    repo: repo.to_path_buf(),
                },
            )
            .with_path(repo.to_path_buf())
            .reason("worktree directories already deleted; prune git metadata"),
        );
    }

    if cfg.worktrees.fetch {
        let _ = git(repo, &["fetch", "--prune", "--quiet", "origin"]);
    }
    let default_ref = default_branch_ref(repo);
    let merged_prs = if gh { merged_prs(repo) } else { HashMap::new() };

    for wt in wts.iter().skip(1) {
        if wt.prunable {
            continue;
        }
        let Some(branch) = &wt.branch else { continue };
        if let Some(c) = judge(
            cfg,
            repo,
            wt,
            branch,
            default_ref.as_deref(),
            &merged_prs,
            orca,
        ) {
            out.push(c);
        }
    }
    out
}

fn judge(
    cfg: &Config,
    repo: &Path,
    wt: &Worktree,
    branch: &str,
    default_ref: Option<&str>,
    merged_prs: &HashMap<String, Vec<MergedPr>>,
    orca: Option<&HashSet<PathBuf>>,
) -> Option<Candidate> {
    if Some(branch) == default_ref.and_then(|r| r.strip_prefix("origin/")) {
        return None;
    }

    // 머지 증거 찾기
    let mut evidence: Option<String> = None;
    let mut suspicious: Option<String> = None;
    if let Some(prs) = merged_prs.get(branch) {
        for pr in prs {
            if pr.head_ref_oid == wt.head
                || is_ancestor(repo, &wt.head, &pr.head_ref_oid, Some(pr.number))
            {
                evidence = Some(format!("PR #{} merged", pr.number));
                break;
            }
            suspicious = Some(format!(
                "PR #{} merged but worktree has newer commits",
                pr.number
            ));
        }
    }
    let gone = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(upstream:track)",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_some_and(|t| t == "[gone]");
    if evidence.is_none()
        && gone
        && let Some(d) = default_ref
    {
        if is_ancestor(repo, &wt.head, d, None) {
            evidence = Some(format!("merged into {d}, remote branch deleted"));
        } else {
            suspicious = suspicious.or(Some(
                "remote branch deleted but merge not verifiable (squash?)".into(),
            ));
        }
    }
    if evidence.is_none() && suspicious.is_none() {
        return None; // 진행 중인 작업
    }

    let is_orca = orca.is_none_or(|set| set.contains(&wt.path));
    let mut cand = Candidate::new(
        Category::Worktree,
        if is_orca {
            "orca-worktree"
        } else {
            "git-worktree"
        },
        format!("worktree:{}", wt.path.display()),
        Safety::Safe,
        Action::RemoveWorktree {
            repo: repo.to_path_buf(),
            path: wt.path.clone(),
            branch: cfg.worktrees.delete_branch.then(|| branch.to_string()),
            orca: is_orca,
        },
    )
    .with_path(wt.path.clone())
    .sized_by(vec![wt.path.clone()]);

    let mut notes = vec![format!("branch {branch}")];
    let blocked = |c: &mut Candidate, why: String| {
        c.safety = Safety::Blocked;
        c.action = Action::None;
        why
    };
    match &evidence {
        Some(e) => notes.push(e.clone()),
        None => {
            cand.safety = Safety::Review;
            notes.push(suspicious.unwrap_or_default());
        }
    }
    if wt.locked {
        notes.push(blocked(&mut cand, "worktree is locked".into()));
    }
    // index mtime 은 git status 가 갱신할 수 있으니 status 보다 먼저 읽는다
    let idle_h = super::git_index_of(&wt.path)
        .and_then(|i| mtime(&i))
        .map(|t| age_days(t) * 24.0)
        .unwrap_or(f64::MAX);
    let dirty = git(&wt.path, &["--no-optional-locks", "status", "--porcelain"])
        .is_none_or(|s| !s.is_empty());
    if dirty {
        notes.push(blocked(&mut cand, "has uncommitted changes".into()));
    }
    if cand.safety == Safety::Safe && idle_h < cfg.worktrees.min_age_hours {
        cand.safety = Safety::Review;
        notes.push(format!("touched {idle_h:.0}h ago"));
    }
    Some(cand.reason(notes.join("; ")))
}

fn default_branch_ref(repo: &Path) -> Option<String> {
    if let Some(r) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return Some(r);
    }
    ["origin/main", "origin/master"]
        .into_iter()
        .find(|r| git_status_ok(repo, &["rev-parse", "--verify", "--quiet", r]))
        .map(String::from)
}

fn merged_prs(repo: &Path) -> HashMap<String, Vec<MergedPr>> {
    let Some(dir) = repo.to_str() else {
        return HashMap::new();
    };
    let out = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "merged",
            "--limit",
            "300",
            "--json",
            "number,headRefName,headRefOid",
        ])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else { return HashMap::new() };
    if !out.status.success() {
        return HashMap::new();
    }
    let prs: Vec<MergedPr> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let mut map: HashMap<String, Vec<MergedPr>> = HashMap::new();
    for pr in prs {
        map.entry(pr.head_ref_name.clone()).or_default().push(pr);
    }
    map
}

/// `a` 가 `b` 의 조상(또는 같음)인지. `b` 커밋이 로컬에 없으면 PR ref 를 받아 온다.
fn is_ancestor(repo: &Path, a: &str, b: &str, pr: Option<u64>) -> bool {
    if !git_status_ok(repo, &["cat-file", "-e", &format!("{b}^{{commit}}")]) {
        let Some(n) = pr else { return false };
        let _ = git(
            repo,
            &["fetch", "--quiet", "origin", &format!("pull/{n}/head")],
        );
    }
    git_status_ok(repo, &["merge-base", "--is-ancestor", a, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_porcelain() {
        let text = "worktree /r\nHEAD aaa\nbranch refs/heads/main\n\n\
                    worktree /r-feat\nHEAD bbb\nbranch refs/heads/feat/x\nlocked\n\n\
                    worktree /gone\nHEAD ccc\ndetached\nprunable gitdir file points to non-existent location\n";
        let wts = parse_porcelain(text);
        assert_eq!(wts.len(), 3);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert_eq!(wts[1].branch.as_deref(), Some("feat/x"));
        assert!(wts[1].locked);
        assert!(wts[2].branch.is_none());
        assert!(wts[2].prunable);
    }
}
