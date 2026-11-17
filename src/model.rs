use std::path::PathBuf;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Worktree,
    Artifact,
    Cache,
    Docker,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Worktree,
        Category::Artifact,
        Category::Cache,
        Category::Docker,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Worktree => "worktree",
            Category::Artifact => "artifact",
            Category::Cache => "cache",
            Category::Docker => "docker",
        }
    }
}

/// 자동 실행에서 지워도 되는지.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// 재생성 가능하고 잃을 것이 없다. `--auto` 에서 지운다.
    Safe,
    /// 지워도 되지만 재다운로드 비용이 크거나 판단이 필요하다. 명시적으로 요청할 때만 지운다.
    Review,
    /// 찾았지만 지우지 않는다 (커밋 안 된 변경 등). 참고용.
    Blocked,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    RemoveDirs {
        paths: Vec<PathBuf>,
    },
    RemoveWorktree {
        repo: PathBuf,
        path: PathBuf,
        branch: Option<String>,
        orca: bool,
    },
    PruneWorktrees {
        repo: PathBuf,
    },
    Run {
        program: String,
        args: Vec<String>,
    },
    None,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    /// 안정적인 식별자. `clean --id` 와 MCP 에서 쓴다.
    pub id: String,
    pub category: Category,
    pub kind: String,
    pub path: Option<PathBuf>,
    pub bytes: Option<u64>,
    /// bytes 가 실제로 비워질 양이 아니라 상한(캐시 전체 크기 등)이면 true.
    pub estimate: bool,
    pub safety: Safety,
    pub reason: String,
    pub action: Action,
    /// 크기를 잴 경로. 스캔 마지막에 병렬로 잰다.
    #[serde(skip)]
    pub size_paths: Vec<PathBuf>,
}

impl Candidate {
    pub fn new(category: Category, kind: &str, id: String, safety: Safety, action: Action) -> Self {
        Candidate {
            id,
            category,
            kind: kind.to_string(),
            path: None,
            bytes: None,
            estimate: false,
            safety,
            reason: String::new(),
            action,
            size_paths: Vec::new(),
        }
    }

    pub fn with_path(mut self, p: PathBuf) -> Self {
        self.path = Some(p);
        self
    }

    pub fn reason(mut self, r: impl Into<String>) -> Self {
        self.reason = r.into();
        self
    }

    pub fn sized_by(mut self, paths: Vec<PathBuf>) -> Self {
        self.size_paths = paths;
        self
    }

    pub fn estimate(mut self) -> Self {
        self.estimate = true;
        self
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub id: String,
    pub kind: String,
    pub ok: bool,
    pub freed: Option<u64>,
    pub message: String,
}
