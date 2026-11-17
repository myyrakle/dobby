use serde_json::{Value, json};

use crate::model::{Candidate, Category, Outcome, Safety};
use crate::util::{home, human};

fn total(cands: &[Candidate], safety: Safety) -> u64 {
    cands
        .iter()
        .filter(|c| c.safety == safety)
        .filter_map(|c| c.bytes)
        .sum()
}

pub fn scan_json(cands: &[Candidate]) -> Value {
    json!({
        "summary": {
            "safe_bytes": total(cands, Safety::Safe),
            "review_bytes": total(cands, Safety::Review),
            "safe": human(total(cands, Safety::Safe)),
            "review": human(total(cands, Safety::Review)),
            "count": cands.len(),
        },
        "candidates": cands,
    })
}

pub fn outcomes_json(outs: &[Outcome]) -> Value {
    let freed: u64 = outs.iter().filter_map(|o| o.freed).sum();
    json!({
        "freed_bytes": freed,
        "freed": human(freed),
        "failed": outs.iter().filter(|o| !o.ok).count(),
        "results": outs,
    })
}

fn short(p: &std::path::Path) -> String {
    match p.strip_prefix(home()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn size_str(c: &Candidate) -> String {
    match c.bytes {
        Some(b) if c.estimate => format!("≤{}", human(b)),
        Some(b) => human(b),
        None => "-".into(),
    }
}

pub fn print_scan(cands: &[Candidate]) {
    if cands.is_empty() {
        println!("nothing to clean 🧦");
        return;
    }
    for cat in Category::ALL {
        let group: Vec<_> = cands.iter().filter(|c| c.category == cat).collect();
        if group.is_empty() {
            continue;
        }
        println!("\n[{}]", cat.label());
        for c in group {
            let mark = match c.safety {
                Safety::Safe => "safe  ",
                Safety::Review => "review",
                Safety::Blocked => "block ",
            };
            let loc = c.path.as_deref().map(short).unwrap_or_default();
            println!("  {mark} {:>10}  {:<22} {loc}", size_str(c), c.kind);
            if !c.reason.is_empty() {
                println!("  {:>18}  └ {}", "", c.reason);
            }
        }
    }
    println!(
        "\nsafe: {}   review: {}   (safe = cleaned by `clean --yes` / schedule; review needs --include-review or --id)",
        human(total(cands, Safety::Safe)),
        human(total(cands, Safety::Review)),
    );
}

pub fn print_outcomes(outs: &[Outcome]) {
    for o in outs {
        let status = if o.ok { "ok  " } else { "FAIL" };
        let freed = o.freed.map(human).unwrap_or_else(|| "-".into());
        println!("  {status} {freed:>10}  {}  {}", o.id, o.message);
    }
    let freed: u64 = outs.iter().filter_map(|o| o.freed).sum();
    let failed = outs.iter().filter(|o| !o.ok).count();
    println!(
        "freed {} ({} actions, {failed} failed)",
        human(freed),
        outs.len()
    );
}
