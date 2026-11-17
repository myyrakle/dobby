use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::{expand, home};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 빌드 산출물과 git 저장소를 찾을 루트 디렉토리들.
    pub roots: Vec<String>,
    /// 루트 아래로 내려갈 최대 깊이.
    pub max_depth: usize,
    /// 이 경로(접두사)들은 건너뛴다.
    pub exclude: Vec<String>,
    pub artifacts: ArtifactsConfig,
    pub worktrees: WorktreesConfig,
    pub caches: CachesConfig,
    pub docker: DockerConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactsConfig {
    pub enabled: bool,
    /// 프로젝트가 이 기간 이상 건드려지지 않았을 때만 산출물을 지운다.
    pub min_age_days: f64,
    /// 이보다 작은 산출물은 보고하지 않는다.
    pub min_size_mb: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreesConfig {
    pub enabled: bool,
    /// 판정 전에 `git fetch --prune` 을 돌린다.
    pub fetch: bool,
    /// GitHub PR 상태로 squash/rebase 머지를 판정한다 (`gh` 필요).
    pub use_gh: bool,
    /// worktree 를 지울 때 머지가 확인된 로컬 브랜치도 지운다.
    pub delete_branch: bool,
    /// 마지막 활동 후 이 시간이 지나야 지운다.
    pub min_age_hours: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CachesConfig {
    pub enabled: bool,
    /// 끌 캐시 kind 목록 (예: ["go-mod", "gradle"]).
    pub disabled: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DockerConfig {
    pub enabled: bool,
    /// 이 기간보다 오래된 빌드 캐시/이미지/컨테이너만 정리한다.
    pub min_age_hours: u64,
}

impl Default for Config {
    fn default() -> Self {
        let candidates = [
            "~/orca",
            "~/Codes",
            "~/code",
            "~/projects",
            "~/dev",
            "~/src",
            "~/workspace",
        ];
        let mut roots: Vec<String> = candidates
            .iter()
            .filter(|r| expand(r).is_dir())
            .map(|r| r.to_string())
            .collect();
        if roots.is_empty() {
            roots.push("~".to_string());
        }
        Config {
            roots,
            max_depth: 6,
            exclude: Vec::new(),
            artifacts: ArtifactsConfig::default(),
            worktrees: WorktreesConfig::default(),
            caches: CachesConfig::default(),
            docker: DockerConfig::default(),
        }
    }
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        ArtifactsConfig {
            enabled: true,
            min_age_days: 14.0,
            min_size_mb: 1,
        }
    }
}

impl Default for WorktreesConfig {
    fn default() -> Self {
        WorktreesConfig {
            enabled: true,
            fetch: true,
            use_gh: true,
            delete_branch: true,
            min_age_hours: 24.0,
        }
    }
}

impl Default for CachesConfig {
    fn default() -> Self {
        CachesConfig {
            enabled: true,
            disabled: Vec::new(),
        }
    }
}

impl Default for DockerConfig {
    fn default() -> Self {
        DockerConfig {
            enabled: true,
            min_age_hours: 24 * 7,
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("dobby").join("config.toml")
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| expand(r)).collect()
    }

    pub fn is_excluded(&self, p: &std::path::Path) -> bool {
        self.exclude.iter().any(|e| p.starts_with(expand(e)))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_fills_defaults() {
        let c: Config =
            toml::from_str("roots = [\"/tmp\"]\n[artifacts]\nmin_age_days = 3\n").unwrap();
        assert_eq!(c.roots, vec!["/tmp"]);
        assert_eq!(c.artifacts.min_age_days, 3.0);
        assert!(c.artifacts.enabled);
        assert!(c.worktrees.use_gh);
    }

    #[test]
    fn default_roundtrips() {
        let c = Config::default();
        let back: Config = toml::from_str(&c.to_toml()).unwrap();
        assert_eq!(back.roots, c.roots);
    }
}
