use std::collections::HashMap;

use crate::config::Config;
use crate::model::{Action, Candidate, Category, Safety};
use crate::util::{has_cmd, parse_size, run_ok};

/// `docker system df --format json` 의 줄 단위 출력에서 Type → (Size, Reclaimable).
pub fn parse_df(text: &str) -> HashMap<String, (u64, u64)> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            let ty = v.get("Type")?.as_str()?.to_string();
            let size = parse_size(v.get("Size")?.as_str()?).unwrap_or(0);
            let recl = parse_size(v.get("Reclaimable")?.as_str()?).unwrap_or(0);
            Some((ty, (size, recl)))
        })
        .collect()
}

pub fn scan(cfg: &Config) -> Vec<Candidate> {
    if !has_cmd("docker") {
        return Vec::new();
    }
    // 데몬이 꺼져 있으면 실패한다
    let Some(text) = run_ok("docker", &["system", "df", "--format", "json"]) else {
        return Vec::new();
    };
    let df = parse_df(&text);
    let until = format!("until={}h", cfg.docker.min_age_hours);
    let mk = |kind: &str, safety, args: &[&str], bytes: u64, reason: String| {
        let mut c = Candidate::new(
            Category::Docker,
            kind,
            format!("docker:{kind}"),
            safety,
            Action::Run {
                program: "docker".into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
        )
        .reason(reason)
        .estimate();
        c.bytes = (bytes > 0).then_some(bytes);
        c
    };

    let mut out = Vec::new();
    let age = cfg.docker.min_age_hours;
    if let Some((size, _)) = df.get("Build Cache") {
        out.push(mk(
            "build-cache",
            Safety::Safe,
            &["builder", "prune", "-f", "--filter", &until],
            *size,
            format!("build cache unused for {age}h+"),
        ));
    }
    if let Some((_, recl)) = df.get("Images") {
        out.push(mk(
            "dangling-images",
            Safety::Safe,
            &["image", "prune", "-f"],
            0,
            "untagged (<none>) images".into(),
        ));
        out.push(mk(
            "unused-images",
            Safety::Review,
            &["image", "prune", "-a", "-f", "--filter", &until],
            *recl,
            format!("images not used by any container, created {age}h+ ago"),
        ));
    }
    if let Some((_, recl)) = df.get("Containers") {
        out.push(mk(
            "stopped-containers",
            Safety::Review,
            &["container", "prune", "-f", "--filter", &until],
            *recl,
            format!("containers stopped {age}h+ ago"),
        ));
    }
    if let Some((_, recl)) = df.get("Local Volumes") {
        out.push(mk(
            "unused-volumes",
            Safety::Review,
            &["volume", "prune", "-f"],
            *recl,
            "volumes not attached to any container (may hold data!)".into(),
        ));
    }
    // dangling 은 크기를 따로 알 수 없으니 0 이어도 남긴다
    out.retain(|c| c.kind == "dangling-images" || c.bytes.is_some());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_df() {
        let text = r#"{"Active":"22","Reclaimable":"6.949GB (36%)","Size":"18.94GB","TotalCount":"76","Type":"Images"}
{"Active":"0","Reclaimable":"270.3kB","Size":"2.292GB","TotalCount":"86","Type":"Build Cache"}"#;
        let df = parse_df(text);
        assert_eq!(df["Images"], (18_940_000_000, 6_949_000_000));
        assert_eq!(df["Build Cache"], (2_292_000_000, 270_300));
    }
}
