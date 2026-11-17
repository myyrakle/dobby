mod clean;
mod config;
mod mcp;
mod memory;
mod model;
mod report;
mod scan;
mod schedule;
mod util;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use config::Config;
use model::{Candidate, Category, Safety};

/// dobby: 로컬 개발 환경 청소부. 머지된 worktree, 오래된 빌드 산출물, 패키지 캐시, docker 찌꺼기를 치운다.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 지울 수 있는 것들을 찾아 보여준다 (아무것도 지우지 않음).
    Scan {
        /// 이 범주만 (반복 가능).
        #[arg(short, long = "category", value_enum)]
        categories: Vec<Category>,
        #[arg(long)]
        json: bool,
    },
    /// 정리한다. --yes 없이는 무엇을 지울지 보여주기만 한다.
    Clean {
        #[arg(short, long = "category", value_enum)]
        categories: Vec<Category>,
        /// scan 에 나온 id 만 지운다 (review 항목도 포함 가능).
        #[arg(long = "id")]
        ids: Vec<String>,
        /// review 항목도 지운다.
        #[arg(long)]
        include_review: bool,
        /// 실제로 지운다.
        #[arg(short, long)]
        yes: bool,
        /// 스케줄러용: safe 항목만, 확인 없이, 로그 형식으로.
        #[arg(long, conflicts_with_all = ["yes", "include_review", "ids"])]
        auto: bool,
        #[arg(long)]
        json: bool,
    },
    /// 메모리 사용 현황을 보여주고, 선택적으로 파일 시스템 캐시를 비운다.
    Memory {
        #[arg(long, default_value_t = 15)]
        top: usize,
        /// 파일 시스템 캐시 비우기 (passwordless sudo 필요).
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        json: bool,
    },
    /// 주기적인 자동 정리 (macOS: launchd, Linux: systemd user timer).
    Schedule {
        #[command(subcommand)]
        action: ScheduleCmd,
    },
    /// stdio MCP 서버로 동작한다 (LLM 에서 scan/clean 호출).
    Mcp,
    /// 설정 파일 관리.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum ScheduleCmd {
    /// 매일 지정 시각에 `dobby clean --auto` 를 실행하도록 등록한다.
    Install {
        /// HH:MM (로컬 시각)
        #[arg(long, default_value = "12:30")]
        at: String,
    },
    Uninstall,
    Status,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// 기본 설정 파일을 만든다.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// 현재 적용되는 설정을 출력한다.
    Show,
    /// 설정 파일 경로.
    Path,
}

/// 지울 대상을 고른다. id 를 지정하면 그것만(review 포함), 아니면 안전 등급으로 거른다.
pub fn plan(
    cfg: &Config,
    cats: &[Category],
    ids: &[String],
    include_review: bool,
) -> Vec<Candidate> {
    let mut cats = cats.to_vec();
    if cats.is_empty() && !ids.is_empty() {
        // id 접두사로 필요한 범주만 스캔한다
        for id in ids {
            let cat = match id.split(':').next().unwrap_or_default() {
                "worktree" | "worktree-prune" => Category::Worktree,
                "artifact" => Category::Artifact,
                "cache" => Category::Cache,
                "docker" => Category::Docker,
                _ => continue,
            };
            if !cats.contains(&cat) {
                cats.push(cat);
            }
        }
    }
    scan::scan(cfg, &cats)
        .into_iter()
        .filter(|c| {
            if ids.is_empty() {
                clean::eligible(c, include_review)
            } else {
                ids.contains(&c.id) && c.safety != Safety::Blocked
            }
        })
        .collect()
}

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn parse_hhmm(s: &str) -> Result<(u8, u8)> {
    let (h, m) = s.split_once(':').context("expected HH:MM")?;
    Ok((h.parse()?, m.parse()?))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Scan { categories, json } => {
            let cfg = Config::load()?;
            let cands = scan::scan(&cfg, &categories);
            if json {
                print_json(&report::scan_json(&cands));
            } else {
                report::print_scan(&cands);
            }
        }
        Cmd::Clean {
            categories,
            ids,
            include_review,
            yes,
            auto,
            json,
        } => {
            let cfg = Config::load()?;
            if auto {
                println!("== dobby clean --auto @ {}", util::now_stamp());
            }
            let targets = plan(&cfg, &categories, &ids, include_review);
            if !(yes || auto) {
                if json {
                    print_json(
                        &serde_json::json!({"dry_run": true, "would_clean": report::scan_json(&targets)}),
                    );
                } else {
                    report::print_scan(&targets);
                    if !targets.is_empty() {
                        println!("\ndry run. re-run with --yes to delete the items above.");
                    }
                }
                return Ok(());
            }
            let outcomes = clean::execute(&targets, true);
            if json {
                print_json(&report::outcomes_json(&outcomes));
            } else {
                report::print_outcomes(&outcomes);
            }
        }
        Cmd::Memory { top, purge, json } => {
            let rep = memory::report(top);
            let purged = purge.then(memory::purge);
            if json {
                let mut v = serde_json::to_value(&rep)?;
                if let Some(p) = &purged {
                    v["purge"] = serde_json::to_value(p)?;
                }
                print_json(&v);
            } else {
                if let (Some(t), Some(a)) = (rep.total_bytes, rep.available_bytes) {
                    println!("memory: {} available of {}", util::human(a), util::human(t));
                }
                for p in &rep.top {
                    println!(
                        "  {:>10}  {:>7}  {}",
                        util::human(p.rss_bytes),
                        p.pid,
                        p.command
                    );
                }
                if let Some(p) = purged {
                    println!("purge: {}", p.message);
                }
            }
        }
        Cmd::Schedule { action } => match action {
            ScheduleCmd::Install { at } => {
                let (h, m) = parse_hhmm(&at)?;
                println!("{}", schedule::install(h, m)?);
            }
            ScheduleCmd::Uninstall => println!("{}", schedule::uninstall()?),
            ScheduleCmd::Status => println!("{}", schedule::status()),
        },
        Cmd::Mcp => mcp::serve()?,
        Cmd::Config { action } => match action {
            ConfigCmd::Init { force } => {
                let p = config::config_path();
                if p.exists() && !force {
                    anyhow::bail!("{} already exists (use --force to overwrite)", p.display());
                }
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::write(&p, Config::default().to_toml())?;
                println!("wrote {}", p.display());
            }
            ConfigCmd::Show => print!("{}", Config::load()?.to_toml()),
            ConfigCmd::Path => println!("{}", config::config_path().display()),
        },
    }
    Ok(())
}
