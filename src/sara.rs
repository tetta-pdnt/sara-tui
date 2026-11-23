use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

#[derive(Deserialize)]
struct SearchPage {
    hits: Vec<SearchHit>,
    next_offset: Option<usize>,
}

#[derive(Deserialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub item: ItemSummary,
    #[serde(rename = "type")]
    pub type_id: String,
    pub file: PathBuf,
}

#[derive(Deserialize)]
struct Schema {
    item_types: Vec<ItemType>,
}

#[derive(Deserialize)]
pub struct ItemType {
    pub id: String,
    pub display_name: String,
}

#[derive(Deserialize)]
pub struct ItemSummary {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub type_name: String,
    pub file: String,
    pub description: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, Field>,
    #[serde(default)]
    pub links: Vec<Relation>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Field {
    Text(String),
    List(Vec<String>),
}

#[derive(Deserialize)]
pub struct Relation {
    pub label: String,
    pub direction: Option<Direction>,
    pub items: Vec<ItemSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upstream,
    Downstream,
    Peer,
}

#[derive(Deserialize)]
pub struct Validation {
    pub valid: bool,
    pub items_checked: usize,
    pub relationships_checked: usize,
    pub error_count: usize,
    pub warning_count: usize,
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub scan_warnings: Vec<ScanWarning>,
}

#[derive(Deserialize)]
pub struct Issue {
    pub severity: String,
    pub message: String,
}

#[derive(Deserialize)]
pub struct ScanWarning {
    pub path: String,
    pub reason: String,
}

pub struct Sara {
    pub project: PathBuf,
}

impl Sara {
    pub fn item_types(&self) -> Result<Vec<ItemType>> {
        let output = Command::new("sara")
            .arg("schema")
            .current_dir(&self.project)
            .output()
            .context("sara schema を起動できません。sara が PATH にあるか確認してください")?;
        if !output.status.success() {
            bail!(
                "Sara のスキーマを取得できません（{}）\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let schema: Schema = serde_yaml_ng::from_slice(&output.stdout)
            .context("Sara のスキーマを読み取れませんでした")?;
        Ok(schema.item_types)
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let mut hits = Vec::new();
        let mut offset = 0;
        loop {
            let page: SearchPage = self.call(
                "sara_search_items",
                json!({"query": query, "offset": offset, "limit": 100}),
            )?;
            hits.extend(page.hits);
            match page.next_offset {
                Some(next) if next > offset => offset = next,
                Some(_) => bail!("Sara の検索結果のページ順序が不正です"),
                None => return Ok(hits),
            }
        }
    }

    pub fn item(&self, id: &str) -> Result<Item> {
        self.call("sara_get_item", json!({"id": id}))
    }

    pub fn validate(&self) -> Result<Validation> {
        self.call("sara_validate", json!({}))
    }

    fn call<T: DeserializeOwned>(&self, name: &str, arguments: Value) -> Result<T> {
        let messages = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "sara-tui", "version": env!("CARGO_PKG_VERSION")}
            }}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": name, "arguments": arguments
            }}),
        ];

        let mut child = Command::new("sara")
            .arg("mcp")
            .current_dir(&self.project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("sara mcp を起動できません。sara が PATH にあるか確認してください")?;
        let mut stdin = child.stdin.take().context("Sara の標準入力がありません")?;
        let sent = (|| -> std::io::Result<()> {
            for message in messages {
                writeln!(stdin, "{message}")?;
            }
            Ok(())
        })();
        drop(stdin);
        let output = child
            .wait_with_output()
            .context("Sara の実行に失敗しました")?;
        if !output.status.success() {
            bail!(
                "Sara の実行に失敗しました（{}）\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        sent.context("Sara にリクエストを送信できませんでした")?;

        let stdout =
            String::from_utf8(output.stdout).context("Sara の応答が UTF-8 ではありません")?;
        for line in stdout.lines() {
            let message: Value =
                serde_json::from_str(line).context("Sara の応答が JSON ではありません")?;
            if message["id"] != 2 {
                continue;
            }
            if let Some(error) = message["error"]["message"].as_str() {
                bail!("{error}");
            }
            let result = &message["result"];
            if result["isError"] == true {
                bail!(
                    "{}",
                    result["content"][0]["text"]
                        .as_str()
                        .unwrap_or("Sara の処理に失敗しました")
                );
            }
            return T::deserialize(&result["structuredContent"])
                .with_context(|| format!("{name} の応答を読み取れませんでした"));
        }
        bail!("{name} の応答がありません")
    }
}
