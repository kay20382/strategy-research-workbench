//! Isolated Git history for portable strategy snapshots.
use crate::{strategy_library::Document, workbench};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const SNAPSHOT: &str = "strategy.json";
const SOURCE: &str = "strategy.pl";
const EMPTY: &str = "0000000000000000000000000000000000000000";
const PARAMETERS: &str = "pyramiding max_position_contracts max_entry_batches interval_seconds interval_days simple_variables function_storage_types source_interval_minutes tick_dataset_id build_volume_on strategy_name price_rounding price_scale minimum_move mc_order_priority_mode mc_open_entry_max_profit_net intrabar_policy sessions dataset_id from to interval_minutes bar_anchor_minutes timestamp_role warmup_bars initial_capital tick_size point_value commission_per_side slippage_ticks contracts margin_per_contract enforce_cash_balance inputs functions research_note";
const MARKET: &str = "symbol unit period session_profile resolved_kind resolved_dataset_id";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    schema: String,
    name: String,
    kind: String,
    #[serde(default)]
    source: String,
    parameters: Value,
    market: Value,
}

fn fields(value: &Value, allowed: &str) -> Result<(), String> {
    let object = value.as_object().ok_or("版本設定須為 JSON 物件")?;
    if let Some(key) = object
        .keys()
        .find(|key| !allowed.split_whitespace().any(|field| field == *key))
    {
        return Err(format!("版本不支援設定欄位：{key}"));
    }
    Ok(())
}

impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        if self.schema != "strategy-version-v1"
            || self.kind != "strategy"
            || self.source.trim().is_empty()
            || !workbench::valid_filename(&self.name)
        {
            return Err("策略版本格式或名稱無效".into());
        }
        fields(&self.parameters, PARAMETERS)?;
        if !self.market.is_null() {
            fields(&self.market, MARKET)?;
        }
        for field in ["inputs", "functions"] {
            if let Some(value) = self.parameters.get(field) {
                let entries = value.as_object().ok_or("策略輸入與函式須為 JSON 物件")?;
                if entries.values().any(|v| {
                    if field == "inputs" {
                        !v.is_number()
                    } else {
                        !v.is_string()
                    }
                }) {
                    return Err("策略輸入或相依函式格式無效".into());
                }
            }
        }
        Ok(())
    }

    fn capture(root: &Path, document: &Document) -> Result<Self, String> {
        let mut snapshot = Self {
            schema: "strategy-version-v1".into(),
            name: document.name.clone(),
            kind: document.kind.clone(),
            source: document.source.clone(),
            parameters: document.task["parameters"].clone(),
            market: document.task["market"].clone(),
        };
        snapshot.validate()?;
        if let Some(files) = document.task["function_files"]
            .as_object()
            .filter(|files| !files.is_empty())
        {
            if snapshot.parameters["functions"]
                .as_object()
                .is_some_and(|entries| !entries.is_empty())
            {
                return Err("函式檔案與內嵌函式不可同時使用".into());
            }
            let mut functions = serde_json::Map::new();
            for (name, file) in files {
                let name = name.to_ascii_lowercase();
                let file = file.as_str().ok_or("函式路徑須為文字")?;
                let source = fs::read_to_string(workbench::absolute(root, file))
                    .map_err(|e| e.to_string())?;
                if functions.insert(name, json!(source)).is_some() {
                    return Err("函式名稱不分大小寫重複".into());
                }
            }
            snapshot.parameters["functions"] = json!(functions);
        }
        // Derived IDs belong to the destination database and will be resolved during backtest.
        if let Some(market) = snapshot.market.as_object_mut() {
            market.remove("resolved_kind");
            market.remove("resolved_dataset_id");
            snapshot
                .parameters
                .as_object_mut()
                .unwrap()
                .remove("dataset_id");
        }
        Ok(snapshot)
    }

    fn apply(self, mut document: Document) -> Document {
        document.name = self.name;
        document.source = self.source;
        document.task["parameters"] = self.parameters;
        document.task["market"] = self.market;
        document.task["function_files"] = json!({});
        document.dirty = true;
        document.result = None;
        document
    }

    fn draft(self, document: Document) -> Document {
        let mut draft = self.apply(document);
        draft.id = crate::strategy_library::fresh_id();
        draft.task["strategy"] = Value::Null;
        draft.origin = None;
        draft.managed_id = None;
        draft
    }
}

fn git(args: &[&str], repo: Option<&Path>, input: Option<&[u8]>) -> Result<Output, String> {
    let mut cmd = Command::new("git");
    cmd.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_AUTHOR_NAME", "REA Strategy Workbench")
        .env("GIT_AUTHOR_EMAIL", "rea-strategy@localhost")
        .env("GIT_COMMITTER_NAME", "REA Strategy Workbench")
        .env("GIT_COMMITTER_EMAIL", "rea-strategy@localhost")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=30",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    if let Some(repo) = repo {
        cmd.arg("--git-dir").arg(repo);
    }
    cmd.args(args).stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().map_err(|e| format!("無法啟動 Git：{e}"))?;
    if let Some(input) = input {
        let write = child
            .stdin
            .take()
            .ok_or("Git 無法接收版本內容")?
            .write_all(input);
        if let Err(error) = write {
            let _ = child.wait_with_output();
            return Err(error.to_string());
        }
    }
    child.wait_with_output().map_err(|e| e.to_string())
}

fn checked(output: Output) -> Result<String, String> {
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Git 作業失敗：{}",
            details.trim().chars().take(1200).collect::<String>()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn object_id(id: &str) -> bool {
    id.len() == 40
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

struct History {
    repo: PathBuf,
    branch: String,
}

impl History {
    fn new(root: &Path, document: &Document) -> Result<Self, String> {
        if document.kind != "strategy" {
            return Err("請選擇策略以管理版本".into());
        }
        let id = document.managed_id.as_deref().unwrap_or(&document.id);
        let hash = format!("{:x}", Sha256::digest(id.as_bytes()));
        Ok(Self {
            repo: std::env::var_os("STRATEGY_VERSION_GIT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::strategy_library::library_dir(root).join("git-versions")),
            branch: format!("strategy-{}", &hash[..24]),
        })
    }

    fn exists(&self) -> bool {
        self.repo.join("HEAD").is_file()
    }
    fn initialize(&self) -> Result<(), String> {
        if !self.exists() {
            fs::create_dir_all(self.repo.parent().unwrap()).map_err(|e| e.to_string())?;
            let path = self.repo.to_str().ok_or("策略 Git 儲存路徑無效")?;
            checked(git(
                &[
                    "init",
                    "--bare",
                    "--object-format=sha1",
                    "--template=",
                    path,
                ],
                None,
                None,
            )?)?;
        }
        Ok(())
    }
    fn output(&self, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
        checked(git(args, Some(&self.repo), input)?)
    }
    fn reference(&self) -> String {
        format!("refs/heads/{}", self.branch)
    }
    fn head(&self, reference: &str) -> Result<Option<String>, String> {
        if !self.exists() {
            return Ok(None);
        }
        let output = git(
            &["rev-parse", "--verify", "--quiet", reference],
            Some(&self.repo),
            None,
        )?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        let id = checked(output)?.trim().to_owned();
        if !object_id(&id) {
            return Err("Git 版本識別無效".into());
        }
        Ok(Some(id))
    }
    fn contains(&self, ancestor: &str, tip: &str) -> Result<bool, String> {
        let status = git(
            &["merge-base", "--is-ancestor", ancestor, tip],
            Some(&self.repo),
            None,
        )?;
        if status.status.code() == Some(1) {
            return Ok(false);
        }
        checked(status)?;
        Ok(true)
    }
    fn list(&self) -> Result<Value, String> {
        let mut entries = Vec::new();
        if let Some(head) = self.head(&self.reference())? {
            let result = self.output(
                &["log", "-n", "100", "-z", "--format=%H%x00%aI%x00%s", &head],
                None,
            )?;
            let fields: Vec<_> = result.split_terminator('\0').collect();
            for row in fields.chunks_exact(3) {
                entries.push(json!({"version":row[0],"date":row[1],"message":row[2]}));
            }
        }
        Ok(json!({"branch":self.branch,"versions":entries}))
    }
    fn commit(&self, snapshot: &Snapshot, message: &str) -> Result<Value, String> {
        let message = message.trim();
        if message.is_empty()
            || message.chars().count() > 200
            || message.chars().any(char::is_control)
        {
            return Err("版本說明須為 1 至 200 個字元的單行文字".into());
        }
        self.initialize()?;
        let reference = self.reference();
        let parent = self.head(&reference)?;
        let mut bytes = serde_json::to_vec_pretty(&json!({
            "schema":snapshot.schema,"name":snapshot.name,"kind":snapshot.kind,
            "parameters":snapshot.parameters,"market":snapshot.market,
        }))
        .map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let metadata = self.output(&["hash-object", "-w", "--stdin"], Some(&bytes))?;
        let source = self.output(
            &["hash-object", "-w", "--stdin"],
            Some(snapshot.source.as_bytes()),
        )?;
        let entries = format!(
            "100644 blob {}\t{SNAPSHOT}\n100644 blob {}\t{SOURCE}\n",
            metadata.trim(),
            source.trim()
        );
        let tree = self.output(&["mktree"], Some(entries.as_bytes()))?;
        if let Some(parent) = &parent {
            if self
                .output(&["rev-parse", &format!("{parent}^{{tree}}")], None)?
                .trim()
                == tree.trim()
            {
                return Err("策略與參數和最新版本相同".into());
            }
        }
        let mut args = vec!["commit-tree", tree.trim(), "-F", "-"];
        if let Some(parent) = &parent {
            args.extend(["-p", parent]);
        }
        let commit = self.output(&args, Some(message.as_bytes()))?;
        let version = commit.trim();
        self.output(
            &[
                "update-ref",
                &reference,
                version,
                parent.as_deref().unwrap_or(EMPTY),
            ],
            None,
        )?;
        Ok(json!({"version":version,"branch":self.branch}))
    }
    fn read(&self, version: &str) -> Result<Snapshot, String> {
        let head = self.head(&self.reference())?.ok_or("目前策略還沒有版本")?;
        if !object_id(version) || !self.contains(version, &head)? {
            return Err("版本不屬於目前策略".into());
        }
        self.read_object(version)
    }
    fn read_object(&self, version: &str) -> Result<Snapshot, String> {
        let raw = self.output(&["show", &format!("{version}:{SNAPSHOT}")], None)?;
        let mut snapshot: Snapshot = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        snapshot.source = self.output(&["show", &format!("{version}:{SOURCE}")], None)?;
        snapshot.validate()?;
        Ok(snapshot)
    }
    fn sync(&self, url: &str, branch: &str, push: bool) -> Result<Value, String> {
        self.initialize()?;
        let local = self.reference();
        let remote = format!("refs/heads/{branch}");
        if push {
            let head = self.head(&local)?.ok_or("請先建立本機版本")?;
            self.output(
                &[
                    "push",
                    "--no-verify",
                    "--no-follow-tags",
                    "--no-recurse-submodules",
                    url,
                    &format!("{head}:{remote}"),
                ],
                None,
            )?;
            return Ok(json!({"action":"push","version":head,"branch":branch}));
        }
        let tag = format!("{:x}", Sha256::digest(format!("{url}/{branch}").as_bytes()));
        let target = format!(
            "refs/rea-fetched/{tag}-{}",
            crate::strategy_library::fresh_id()
        );
        self.output(
            &[
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--no-write-fetch-head",
                url,
                &format!("{remote}:{target}"),
            ],
            None,
        )?;
        let result = (|| {
            let fetched = self.head(&target)?.ok_or("GitHub 未回傳版本")?;
            self.read_object(&fetched)?;
            let current = self.head(&local)?;
            if let Some(current) = &current {
                if self.contains(&fetched, current)? {
                    return Ok(
                        json!({"action":"fetch","status":"current_or_ahead","version":current,"branch":branch}),
                    );
                }
                if !self.contains(current, &fetched)? {
                    return Err("本機與 GitHub 歷史分岔，請用另一份策略草稿匯入遠端版本".into());
                }
            }
            self.output(
                &[
                    "update-ref",
                    &local,
                    &fetched,
                    current.as_deref().unwrap_or(EMPTY),
                ],
                None,
            )?;
            Ok(json!({"action":"fetch","status":"updated","version":fetched,"branch":branch}))
        })();
        let cleanup = self.output(&["update-ref", "-d", &target], None);
        result.and_then(|value| cleanup.map(|_| value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_versions_restore_and_sync_without_overwriting_diverged_history() {
        let directory = std::env::temp_dir().join(format!(
            "rea-versions-{}",
            crate::strategy_library::fresh_id()
        ));
        let make_history = |name: &str| History {
            repo: directory.join(name),
            branch: "strategy-fixture".into(),
        };
        let local = make_history("local.git");
        let remote = make_history("remote.git");
        let peer = make_history("peer.git");
        fs::create_dir_all(&directory).unwrap();
        remote.initialize().unwrap();
        let url = remote.repo.to_str().unwrap();
        fs::write(directory.join("indicator.pl"), "Indicator = Close;").unwrap();
        let document = Document {
            id: "fixture".into(),
            name: "測試策略".into(),
            kind: "strategy".into(),
            source: "Buy next bar at market;".into(),
            origin: None,
            managed_id: None,
            dirty: true,
            result: None,
            task: json!({"parameters":{"inputs":{"Length":8},"dataset_id":7},"market":{"symbol":"TX","unit":"minutes","period":1},
                "database":"local-market.sqlite","output":"local-output","function_files":{"Indicator":"indicator.pl"}}),
        };
        let mut snapshot = Snapshot::capture(&directory, &document).unwrap();
        let first = local.commit(&snapshot, "初版").unwrap();
        assert!(local.commit(&snapshot, "內容相同").is_err());
        snapshot.parameters["inputs"]["Length"] = json!(13);
        let second = local.commit(&snapshot, "修改週期").unwrap();
        let list = local.list().unwrap();
        assert_eq!(list["versions"][0]["version"], second["version"]);
        assert_eq!(list["versions"][1]["message"], "初版");
        let draft = local
            .read(first["version"].as_str().unwrap())
            .unwrap()
            .draft(document.clone());
        assert_eq!(draft.task["parameters"]["inputs"]["Length"], 8);
        assert_eq!(
            draft.task["parameters"]["functions"]["indicator"],
            "Indicator = Close;"
        );
        assert_eq!(draft.task["database"], "local-market.sqlite");
        assert!(draft.task["parameters"]["dataset_id"].is_null());
        assert!(draft.dirty && draft.managed_id.is_none());
        local.sync(url, &local.branch, true).unwrap();
        peer.sync(url, &local.branch, false).unwrap();
        assert_eq!(peer.list().unwrap()["versions"], list["versions"]);
        snapshot.parameters["inputs"]["Length"] = json!(21);
        local.commit(&snapshot, "本機調整").unwrap();
        snapshot.parameters["inputs"]["Length"] = json!(34);
        peer.commit(&snapshot, "另一台調整").unwrap();
        local.sync(url, &local.branch, true).unwrap();
        let before = peer.head(&peer.reference()).unwrap();
        assert!(peer.sync(url, &local.branch, false).is_err());
        assert!(peer.sync(url, &local.branch, true).is_err());
        assert_eq!(peer.head(&peer.reference()).unwrap(), before);
        assert!(github("https://user:token@github.com/owner/repo").is_err());
        assert!(github("https://github.com/owner/repo?token=literal").is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}

fn github(value: &str) -> Result<String, String> {
    let path = value
        .trim()
        .strip_prefix("https://github.com/")
        .ok_or("請填入 https://github.com/擁有者/儲存庫")?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path
        .split_once('/')
        .ok_or("GitHub 網址須含擁有者與儲存庫")?;
    let valid = |name: &str| {
        !name.is_empty()
            && name.len() <= 100
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if !valid(owner) || !valid(repo) {
        return Err("GitHub 網址格式無效，請使用 Git 憑證管理員登入".into());
    }
    Ok(format!("https://github.com/{owner}/{repo}.git"))
}

pub fn execute(root: &Path, document: Document, request: &Value) -> Result<Value, String> {
    let history = History::new(root, &document)?;
    let text = |key| request[key].as_str().unwrap_or("");
    match text("operation") {
        "list" => history.list(),
        "commit" => history.commit(&Snapshot::capture(root, &document)?, text("message")),
        "read" => Ok(json!(history.read(text("version"))?)),
        "draft" => Ok(json!(history.read(text("version"))?.draft(document))),
        "apply" if request["confirmed"] == true => Ok(json!({
            "applied_document":history.read(text("version"))?.apply(document)
        })),
        operation @ ("push" | "fetch") => {
            let url = github(text("remote"))?;
            let branch = if text("branch").trim().is_empty() {
                &history.branch
            } else {
                text("branch").trim()
            };
            if !branch.starts_with("strategy-")
                || branch.len() > 100
                || branch.contains("..")
                || branch.ends_with(".lock")
                || !branch
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                return Err("GitHub 分支須以 strategy- 開頭，並使用英數、連字號或底線".into());
            }
            if operation == "push" && request["confirmed"] != true {
                return Err("請先確認推送策略與參數至 GitHub".into());
            }
            history.sync(&url, branch, operation == "push")
        }
        _ => Err("未知的策略版本操作".into()),
    }
}
