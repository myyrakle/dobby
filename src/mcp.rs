//! stdio 위에서 동작하는 최소 MCP 서버 (JSON-RPC 2.0, 줄 단위 메시지).
//! stdout 은 프로토콜 전용이므로 여기서 호출하는 코드는 stdout 에 쓰면 안 된다.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{Value, json};

use crate::config::Config;
use crate::model::Category;

const PROTOCOL_VERSION: &str = "2025-06-18";

fn tools() -> Value {
    let categories = json!({
        "type": "array",
        "items": {"type": "string", "enum": ["worktree", "artifact", "cache", "docker"]},
        "description": "Limit to these categories. Omit for all."
    });
    json!([
        {
            "name": "dobby_scan",
            "description": "Scan the local machine for reclaimable disk space: merged git worktrees, stale build artifacts (target/, node_modules/, ...), package manager caches and docker data. Read-only. Each candidate has an id, size, and safety: 'safe' (regenerable, cleaned automatically), 'review' (needs the user's OK, e.g. full cache wipes, venvs, docker volumes), 'blocked' (never cleaned, e.g. worktree with uncommitted changes).",
            "inputSchema": {"type": "object", "properties": {"categories": categories}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "dobby_clean",
            "description": "Delete what dobby_scan found. Defaults to a dry run; pass dry_run=false to actually delete. Without ids it cleans every 'safe' candidate (plus 'review' ones if include_review=true). Passing ids cleans exactly those candidates, including 'review' ones. 'blocked' candidates are never deleted. Show the user the scan results and get confirmation before deleting 'review' items.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "categories": categories,
                    "ids": {"type": "array", "items": {"type": "string"}, "description": "Candidate ids from dobby_scan."},
                    "include_review": {"type": "boolean", "default": false},
                    "dry_run": {"type": "boolean", "default": true}
                }
            },
            "annotations": {"destructiveHint": true}
        },
        {
            "name": "dobby_memory",
            "description": "Report total/available memory and the processes using the most memory. With purge=true, also flush file system caches (needs passwordless sudo for purge/drop_caches).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "top": {"type": "integer", "default": 15},
                    "purge": {"type": "boolean", "default": false}
                }
            }
        }
    ])
}

fn parse_categories(args: &Value) -> Result<Vec<Category>> {
    let Some(list) = args.get("categories").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    list.iter()
        .map(|v| {
            let s = v.as_str().unwrap_or_default();
            Category::ALL
                .into_iter()
                .find(|c| c.label() == s)
                .ok_or_else(|| anyhow::anyhow!("unknown category {s:?}"))
        })
        .collect()
}

fn call_tool(name: &str, args: &Value) -> Result<Value> {
    let cfg = Config::load()?;
    match name {
        "dobby_scan" => {
            let cands = crate::scan::scan(&cfg, &parse_categories(args)?);
            Ok(crate::report::scan_json(&cands))
        }
        "dobby_clean" => {
            let ids: Vec<String> = args
                .get("ids")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let include_review = args
                .get("include_review")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
            let cats = parse_categories(args)?;
            let plan = crate::plan(&cfg, &cats, &ids, include_review);
            if dry_run {
                return Ok(
                    json!({"dry_run": true, "would_clean": crate::report::scan_json(&plan)}),
                );
            }
            let outcomes = crate::clean::execute(&plan, true);
            Ok(crate::report::outcomes_json(&outcomes))
        }
        "dobby_memory" => {
            let top = args.get("top").and_then(Value::as_u64).unwrap_or(15) as usize;
            let mut v = serde_json::to_value(crate::memory::report(top))?;
            if args.get("purge").and_then(Value::as_bool).unwrap_or(false) {
                v["purge"] = serde_json::to_value(crate::memory::purge())?;
            }
            Ok(v)
        }
        _ => anyhow::bail!("unknown tool {name}"),
    }
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone(); // id 가 없으면 notification: 응답하지 않는다
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "dobby", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "dobby cleans up the local dev machine. Call dobby_scan first, summarize the findings for the user, then call dobby_clean. Only 'safe' items are cleaned by default."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(match call_tool(name, &args) {
                Ok(v) => json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                    "structuredContent": v,
                    "isError": false
                }),
                Err(e) => json!({
                    "content": [{"type": "text", "text": format!("{e:#}")}],
                    "isError": true
                }),
            })
        }
        _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg),
            Err(e) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}}),
            ),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list() {
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 3);
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let r = handle(&json!({"jsonrpc":"2.0","id":3,"method":"nope"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }
}
