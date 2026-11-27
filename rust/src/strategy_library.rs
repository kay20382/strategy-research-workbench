//! 策略／函式清單與獨立草稿。原始還原碼唯讀，儲存寫入工作台專屬資料夾。
use crate::workbench as core;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub source: PathBuf,
    pub task: Option<PathBuf>,
    pub managed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub source: String,
    pub task: Value,
    pub origin: Option<PathBuf>,
    pub managed_id: Option<String>,
    pub dirty: bool,
    pub result: Option<Value>,
}
pub fn library_dir(root: &Path) -> PathBuf {
    root.join("data/strategy-library")
}
pub(crate) fn fresh_id() -> String {
    format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    )
}
fn digest(s: &[u8]) -> String {
    format!("{:x}", Sha256::digest(s))
}
pub fn catalog(root: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = vec![];
    let mut seen = HashSet::new();
    let mut tasks = vec![root.join("tasks/pl-workbench-example.json")];
    let cases = root.join("data/mc-regression-cases.json");
    if cases.exists() {
        let v = core::read_json(&cases)?;
        if let Some(rows) = v["cases"].as_array() {
            for r in rows {
                if let Some(task) = r["task"].as_str() {
                    tasks.push(core::absolute(root, task));
                }
            }
        }
    }
    let native = root.join("tasks/kgi-08-be5-native-parity-backtest.json");
    if native.exists() {
        tasks.push(native);
    }
    for path in tasks {
        let Ok(task) = core::load_task(root, &path) else {
            continue;
        };
        if let Some(source) = task["strategy"].as_str() {
            let p = PathBuf::from(source);
            if p.is_file() {
                let name = task["parameters"]["strategy_name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| p.file_stem().and_then(|s| s.to_str()).unwrap_or("策略"))
                    .trim_start_matches("s_")
                    .to_owned();
                let key = format!("strategy:{}", name.to_lowercase());
                let candidate = Entry {
                    id: digest(key.as_bytes()),
                    name,
                    kind: "strategy".into(),
                    source: p,
                    task: Some(path.clone()),
                    managed: false,
                };
                if seen.insert(key.clone()) {
                    entries.push(candidate);
                } else if let Some(existing) =
                    entries.iter_mut().find(|entry| entry.id == candidate.id)
                {
                    // 固定驗收任務優先；後列的正式原生對照任務更新同名入口，
                    // 原始檔與歷史任務仍各自保留，不在 UI 顯示難以辨識的重名項目。
                    *existing = candidate;
                }
            }
        }
        if let Some(functions) = task["function_files"].as_object() {
            for (name, p) in functions {
                if let Some(p) = p.as_str() {
                    let p = core::absolute(root, p);
                    let key = format!("function:{}", name.to_ascii_lowercase());
                    if p.is_file() && seen.insert(key.clone()) {
                        entries.push(Entry {
                            id: digest(key.as_bytes()),
                            name: name.clone(),
                            kind: "function".into(),
                            source: p,
                            task: Some(path.clone()),
                            managed: false,
                        });
                    }
                }
            }
        }
    }
    let user = root.join("rust/tests/fixtures/pl-workbench/rsi-strategy.pl");
    if user.exists() {
        entries.push(Entry {
            id: "user-kyle-test1".into(),
            name: "_kyleTest1".into(),
            kind: "strategy".into(),
            source: user,
            task: Some(root.join("tasks/pl-workbench-example.json")),
            managed: false,
        });
    }
    for (name, path) in [
        ("REA_EngineOrderMatrix", "tasks/engine-order-matrix.json"),
        ("REA_EngineEdgeCases", "tasks/engine-edge-cases.json"),
        ("REA_ScaleInOut5", "tasks/scale-in-out-5-any-signal.json"),
        ("REA_DifferentSignal5", "tasks/different-signal-5.json"),
        ("REA_SyntheticOrderLab", "tasks/synthetic-order-lab.json"),
    ] {
        let path = root.join(path);
        if path.is_file() && !entries.iter().any(|entry| entry.name == name) {
            let task = core::load_task(root, &path)?;
            entries.push(Entry {
                id: name.into(),
                name: name.into(),
                kind: "strategy".into(),
                source: task["strategy"].as_str().ok_or("缺少策略路徑")?.into(),
                task: Some(path),
                managed: false,
            });
        }
    }
    let index = library_dir(root).join("catalog.json");
    if index.exists() {
        let own: Vec<Entry> =
            serde_json::from_value(core::read_json(&index)?).map_err(|e| e.to_string())?;
        entries.extend(own);
    }
    entries.sort_by_key(|e| (e.kind.clone(), e.name.to_lowercase(), e.id.clone()));
    Ok(entries)
}
pub fn open(root: &Path, entry: &Entry) -> Result<Document, String> {
    let default_task = root.join("tasks/pl-workbench-example.json");
    let mut task = core::load_task(root, entry.task.as_deref().unwrap_or(&default_task))?;
    task["parameters"]["strategy_name"] = json!(entry.name);
    Ok(Document {
        id: entry.id.clone(),
        name: entry.name.clone(),
        kind: entry.kind.clone(),
        source: crate::pl_exchange::read(&entry.source)?,
        task,
        origin: Some(entry.source.clone()),
        managed_id: if entry.managed {
            Some(entry.id.clone())
        } else {
            None
        },
        dirty: false,
        result: None,
    })
}
pub fn new_document(root: &Path, number: usize) -> Result<Document, String> {
    let mut task = core::load_task(root, &root.join("tasks/pl-workbench-example.json"))?;
    let name = format!("新策略{number}");
    task["parameters"]["strategy_name"] = json!(name);
    Ok(Document {
        id: fresh_id(),
        name,
        kind: "strategy".into(),
        source: "{ 在此貼上或撰寫 PL 策略，再按「檢查語法」或「開始回測」。 }\n".into(),
        task,
        origin: None,
        managed_id: None,
        dirty: true,
        result: None,
    })
}
pub fn save(root: &Path, doc: &mut Document) -> Result<Entry, String> {
    if !core::valid_filename(&doc.name) {
        return Err("策略名稱不可空白，且不可含路徑或 Windows 保留字".into());
    }
    let dir = library_dir(root);
    fs::create_dir_all(dir.join("sources")).map_err(|e| e.to_string())?;
    fs::create_dir_all(dir.join("tasks")).map_err(|e| e.to_string())?;
    let id = doc.managed_id.clone().unwrap_or_else(|| {
        // New drafts already have a unique local identity. Keep their version history after saving.
        if doc.id.split_once('-').is_some_and(|(left, right)| {
            [left, right].iter().all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        }) {
            doc.id.clone()
        } else {
            fresh_id()
        }
    });
    let source = dir.join("sources").join(format!("{id}.pl"));
    let taskpath = dir.join("tasks").join(format!("{id}.json"));
    if source.exists() {
        let backup = dir.join("versions").join(fresh_id());
        fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
        fs::copy(&source, backup.join("source.pl")).map_err(|e| e.to_string())?;
        if taskpath.exists() {
            fs::copy(&taskpath, backup.join("task.json")).map_err(|e| e.to_string())?;
        }
    }
    fs::write(&source, &doc.source).map_err(|e| e.to_string())?;
    let mut task = doc.task.clone();
    task["strategy"] = json!(source);
    task["parameters"]["strategy_name"] = json!(doc.name);
    core::write_json(&taskpath, &task)?;
    let entry = Entry {
        id: id.clone(),
        name: doc.name.clone(),
        kind: doc.kind.clone(),
        source: source.clone(),
        task: Some(taskpath),
        managed: true,
    };
    let index = dir.join("catalog.json");
    let mut own: Vec<Entry> = if index.exists() {
        serde_json::from_value(core::read_json(&index)?).map_err(|e| e.to_string())?
    } else {
        vec![]
    };
    own.retain(|e| e.id != id);
    own.push(entry.clone());
    core::write_json(&index, &json!(own))?;
    let origin = doc
        .origin
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    core::write_json(
        &source.with_extension("provenance.json"),
        &json!({"source_origin":origin,"saved_source_sha256":digest(doc.source.as_bytes()),"original_source_modified":false,"saved_as_managed_copy":true}),
    )?;
    doc.managed_id = Some(id);
    doc.dirty = false;
    Ok(entry)
}
pub fn save_session(root: &Path, documents: &[Document], active: usize) -> Result<(), String> {
    let dir = library_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 尚未按儲存的草稿另存，不寫回任何原始策略。
    let tmp = dir.join("session.tmp.json");
    core::write_json(
        &tmp,
        &json!({"schema":"pl-workbench-session-v1","active":active,"documents":documents}),
    )?;
    fs::rename(tmp, dir.join("session.json")).map_err(|e| e.to_string())
}
pub fn restore_session(root: &Path) -> Result<Option<(Vec<Document>, usize)>, String> {
    let file = library_dir(root).join("session.json");
    if !file.exists() {
        return Ok(None);
    }
    let v = core::read_json(&file)?;
    let docs: Vec<Document> =
        serde_json::from_value(v["documents"].clone()).map_err(|e| e.to_string())?;
    if docs.is_empty() {
        return Ok(None);
    }
    let active = v["active"].as_u64().unwrap_or(0) as usize;
    let active = active.min(docs.len() - 1);
    Ok(Some((docs, active)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_save_preserves_original_and_restores_drafts() {
        let root = core::root()
            .join("outputs")
            .join(format!("library-test-{}", fresh_id()));
        fs::create_dir_all(root.join("tasks")).unwrap();
        let original = root.join("original.pl");
        fs::write(&original, "var:x(0); x=1;").unwrap();
        let base = core::read_json(&core::root().join("tasks/pl-workbench-example.json")).unwrap();
        core::write_json(&root.join("tasks/pl-workbench-example.json"), &base).unwrap();
        let entry = Entry {
            id: "original".into(),
            name: "REA_測試".into(),
            kind: "strategy".into(),
            source: original.clone(),
            task: Some(root.join("tasks/pl-workbench-example.json")),
            managed: false,
        };
        let mut d = open(&root, &entry).unwrap();
        d.source = "var:x(0); x=2;".into();
        let saved = save(&root, &mut d).unwrap();
        assert_ne!(saved.source, original);
        assert_eq!(fs::read_to_string(&original).unwrap(), "var:x(0); x=1;");
        d.source = "var:x(0); x=3;".into();
        d.dirty = true;
        save_session(&root, &[d.clone()], 0).unwrap();
        let (docs, index) = restore_session(&root).unwrap().unwrap();
        assert_eq!(index, 0);
        assert_eq!(docs[0].source, d.source);
        assert!(docs[0].dirty);
        let mut other = d.clone();
        other.id = "other-document".into();
        other.name = "REA_另一策略".into();
        other.source = "var:x(0); x=99;".into();
        save_session(&root, &[d.clone(), other.clone()], 1).unwrap();
        let (docs, index) = restore_session(&root).unwrap().unwrap();
        assert_eq!(index, 1);
        assert_eq!(docs[0].source, d.source);
        assert_eq!(docs[1].source, other.source);
        save(&root, &mut d).unwrap();
        assert_eq!(fs::read_to_string(saved.source).unwrap(), d.source);
        assert_eq!(
            fs::read_dir(library_dir(&root).join("versions"))
                .unwrap()
                .count(),
            1
        );
    }
}
