use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::model::{Action, Candidate, Category, Safety};
use crate::util::{age_days, mtime, shallow_mtime};

/// 빌드 산출물 디렉토리를 알아보는 규칙.
pub struct Rule {
    /// 디렉토리 이름.
    pub dir: &'static str,
    /// 부모 디렉토리에 이 중 하나가 있어야 한다. 비어 있으면 조건 없음.
    pub markers: &'static [&'static str],
    /// 산출물 디렉토리 안에 이 파일이 있어야 한다.
    pub inner: Option<&'static str>,
    pub kind: &'static str,
    pub safety: Safety,
}

const JS: &[&str] = &["package.json"];
const GRADLE: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

const fn rule(dir: &'static str, markers: &'static [&'static str], kind: &'static str) -> Rule {
    Rule {
        dir,
        markers,
        inner: None,
        kind,
        safety: Safety::Safe,
    }
}

pub static RULES: &[Rule] = &[
    rule("target", &["Cargo.toml"], "rust-target"),
    rule("node_modules", JS, "node_modules"),
    rule(".next", JS, "next-build"),
    rule(".nuxt", JS, "nuxt-build"),
    rule(".svelte-kit", JS, "sveltekit-build"),
    rule(".turbo", JS, "turbo-cache"),
    rule(".parcel-cache", JS, "parcel-cache"),
    rule(".angular", JS, "angular-cache"),
    rule("__pycache__", &[], "python-bytecode"),
    rule(".pytest_cache", &[], "pytest-cache"),
    rule(".mypy_cache", &[], "mypy-cache"),
    rule(".ruff_cache", &[], "ruff-cache"),
    rule(
        ".tox",
        &["tox.ini", "pyproject.toml", "setup.cfg"],
        "tox-envs",
    ),
    Rule {
        dir: ".venv",
        markers: &[],
        inner: Some("pyvenv.cfg"),
        kind: "python-venv",
        safety: Safety::Review,
    },
    Rule {
        dir: "venv",
        markers: &[],
        inner: Some("pyvenv.cfg"),
        kind: "python-venv",
        safety: Safety::Review,
    },
    rule(".build", &["Package.swift"], "swift-build"),
    rule("build", GRADLE, "gradle-build"),
    rule(".gradle", GRADLE, "gradle-project-cache"),
    rule(".zig-cache", &["build.zig"], "zig-cache"),
    rule("zig-cache", &["build.zig"], "zig-cache"),
    rule("_build", &["mix.exs"], "elixir-build"),
    rule("deps", &["mix.exs"], "elixir-deps"),
    rule(".dart_tool", &["pubspec.yaml"], "dart-tool"),
    rule(".stack-work", &["stack.yaml"], "haskell-stack"),
    rule("dist-newstyle", &[], "haskell-cabal"),
];

pub fn match_rule(parent: &Path, name: &str, path: &Path) -> Option<&'static Rule> {
    RULES.iter().find(|r| {
        r.dir == name
            && (r.markers.is_empty() || r.markers.iter().any(|m| parent.join(m).exists()))
            && r.inner.is_none_or(|f| path.join(f).exists())
    })
}

pub fn scan(cfg: &Config, found: &[(PathBuf, &'static Rule)]) -> Vec<Candidate> {
    found
        .iter()
        .filter_map(|(path, rule)| {
            let project = path.parent()?;
            let activity = [
                shallow_mtime(path),
                shallow_mtime(project),
                super::git_index_of(project).and_then(|i| mtime(&i)),
            ]
            .into_iter()
            .flatten()
            .max()?;
            let idle = age_days(activity);
            if idle < cfg.artifacts.min_age_days {
                return None;
            }
            Some(
                Candidate::new(
                    Category::Artifact,
                    rule.kind,
                    format!("artifact:{}", path.display()),
                    rule.safety,
                    Action::RemoveDirs {
                        paths: vec![path.clone()],
                    },
                )
                .with_path(path.clone())
                .sized_by(vec![path.clone()])
                .reason(format!("project idle for {idle:.0} days")),
            )
        })
        .collect()
}
