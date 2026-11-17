use std::path::{Component, Path};

use crate::model::{Action, Candidate, Outcome, Safety};
use crate::util::{command, dir_size, git, git_status_ok, has_cmd, home, parse_size};

/// 실행 대상인지 판단한다. Blocked 는 절대 실행하지 않는다.
pub fn eligible(c: &Candidate, include_review: bool) -> bool {
    match c.safety {
        Safety::Safe => true,
        Safety::Review => include_review,
        Safety::Blocked => false,
    }
}

pub fn execute(cands: &[Candidate], include_review: bool) -> Vec<Outcome> {
    cands
        .iter()
        .filter(|c| eligible(c, include_review))
        .map(|c| {
            let res = run_one(c);
            let (ok, freed, message) = match res {
                Ok((freed, msg)) => (true, freed, msg),
                Err(e) => (false, None, format!("{e:#}")),
            };
            Outcome {
                id: c.id.clone(),
                kind: c.kind.clone(),
                ok,
                freed,
                message,
            }
        })
        .collect()
}

/// 지워도 되는 경로인지 마지막으로 확인한다: 절대 경로, `..` 없음, 루트/홈이 아니고 충분히 깊음.
pub fn deletable(p: &Path) -> bool {
    let home = home();
    p.is_absolute()
        && !p.components().any(|c| c == Component::ParentDir)
        && p.components().count() >= 4
        && p != home
        && !home.starts_with(p)
}

fn run_one(c: &Candidate) -> anyhow::Result<(Option<u64>, String)> {
    match &c.action {
        Action::None => anyhow::bail!("nothing to do"),
        Action::RemoveDirs { paths } => {
            let mut freed = 0;
            for p in paths.iter().filter(|p| p.exists()) {
                anyhow::ensure!(deletable(p), "refusing to delete {}", p.display());
                let size = dir_size(p);
                std::fs::remove_dir_all(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                freed += size;
            }
            Ok((Some(freed), "removed".into()))
        }
        Action::PruneWorktrees { repo } => {
            anyhow::ensure!(
                git_status_ok(repo, &["worktree", "prune"]),
                "git worktree prune failed"
            );
            Ok((None, "pruned stale worktree metadata".into()))
        }
        Action::RemoveWorktree {
            repo,
            path,
            branch,
            orca,
        } => remove_worktree(repo, path, branch.as_deref(), *orca),
        Action::Run { program, args } => {
            anyhow::ensure!(has_cmd(program), "{program} not found in PATH");
            let before: u64 = c.size_paths.iter().map(|p| dir_size(p)).sum();
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let out = command(program, &args).output()?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !out.status.success() {
                anyhow::bail!(
                    "{program} exited with {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            let freed = if !c.size_paths.is_empty() {
                let after: u64 = c.size_paths.iter().map(|p| dir_size(p)).sum();
                Some(before.saturating_sub(after))
            } else {
                // docker: "Total reclaimed space: 1.2GB"
                stdout
                    .lines()
                    .find_map(|l| l.strip_prefix("Total reclaimed space:"))
                    .and_then(|s| parse_size(s.trim()))
            };
            Ok((freed, format!("ran {program} {}", args.join(" "))))
        }
    }
}

fn remove_worktree(
    repo: &Path,
    path: &Path,
    branch: Option<&str>,
    orca: bool,
) -> anyhow::Result<(Option<u64>, String)> {
    anyhow::ensure!(deletable(path), "refusing to delete {}", path.display());
    // 스캔 이후 변경이 생겼을 수 있으니 다시 확인
    let dirty =
        git(path, &["--no-optional-locks", "status", "--porcelain"]).is_none_or(|s| !s.is_empty());
    anyhow::ensure!(!dirty, "worktree became dirty since scan");
    let size = dir_size(path);

    let mut how = "git";
    let mut removed = false;
    if orca && has_cmd("orca") {
        let selector = format!("path:{}", path.display());
        let ok = command(
            "orca",
            &[
                "worktree",
                "rm",
                "--worktree",
                &selector,
                "--force",
                "--json",
            ],
        )
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
        if ok {
            how = "orca";
            removed = true;
        }
    }
    if !removed {
        let p = path.to_string_lossy();
        anyhow::ensure!(
            git_status_ok(repo, &["worktree", "remove", "--force", &p]),
            "git worktree remove failed"
        );
    }
    let mut msg = format!("removed worktree via {how}");
    if let Some(b) = branch {
        let r = format!("refs/heads/{b}");
        if git_status_ok(repo, &["rev-parse", "--verify", "--quiet", &r]) {
            // 머지는 스캔 단계에서 증명했다 (squash 머지는 -d 로 지워지지 않는다)
            if git_status_ok(repo, &["branch", "-D", b]) {
                msg.push_str(&format!(", deleted branch {b}"));
            } else {
                msg.push_str(&format!(", kept branch {b} (delete failed)"));
            }
        }
    }
    Ok((Some(size), msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn deletable_guards() {
        let h = home();
        assert!(!deletable(&h));
        assert!(!deletable(Path::new("/")));
        assert!(!deletable(Path::new("/Users")));
        assert!(!deletable(&h.join("a/../../..")));
        assert!(!deletable(Path::new("relative/path/x/y")));
        assert!(deletable(&h.join("code/proj/target")));
        assert!(deletable(&PathBuf::from("/tmp/a/b/c")));
    }
}
