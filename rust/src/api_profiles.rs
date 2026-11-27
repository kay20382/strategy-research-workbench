use godot::prelude::*;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub provider: String,
    pub purpose: String,
    #[serde(default)]
    pub connection: String,
    #[serde(default)]
    pub host: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub ca_path: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Credentials {
    pub account: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub ca_password: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    profile: Profile,
    #[serde(default)]
    credentials: Credentials,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaRequest {
    provider: String,
    purpose: String,
}

fn schema(provider: &str, purpose: &str) -> Value {
    let option = |key, label, kind, required| {
        json!({"key":key,"label":label,"kind":kind,"required_for_connection":required})
    };
    let quotecom = provider == "kgi_quotecom" && purpose == "quote";
    if provider == "kgi_tradecom" && purpose == "trade" {
        return json!({
            "provider":provider,"purpose":purpose,"label":"凱基 TradeCom 交易",
            "supports_custom_fields":false,"adapter_available":true,
            "fields":[
                option("account", "使用者 (UserID)", "secret", true),
                option("password", "密碼 (PassWord)", "secret", true),
                option("host", "連線 IP 或主機 (Host)", "text", true),
                option("port", "連接埠 (Port)", "integer", true),
                option("trade_account", "交易帳號 (Account)", "secret", true),
                option("sub_account", "交易子帳 (SubAccount)", "secret", false),
                option("broker_id", "券商代碼 (BrokerID)", "text", true),
            ]
        });
    }
    let mut fields = vec![
        option("account", "使用者 (UserID)", "secret", quotecom),
        option("password", "密碼 (PassWord)", "secret", quotecom),
        option("host", "連線 IP 或主機 (Host)", "text", quotecom),
        option("port", "連接埠 (Port)", "integer", quotecom),
        option("source_id", "來源 (SourceID)", "text", quotecom),
        option("token", "權杖 (Token)", "secret", quotecom),
    ];
    if !quotecom {
        fields.extend([
            option("connection", "固定連線名稱", "text", false),
            option("ca_path", "CA 憑證路徑", "path", false),
            option("ca_password", "CA 密碼", "secret", false),
        ]);
    }
    json!({
        "provider":provider,"purpose":purpose,"fields":fields,
        "label":if quotecom {"凱基 QuoteCom 行情"} else {"自訂券商 API"},
        "supports_custom_fields":!quotecom,"adapter_available":quotecom
    })
}

fn valid_option_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn sql<T>(value: rusqlite::Result<T>) -> Result<T, String> {
    value.map_err(|e| e.to_string())
}

pub fn location(root: &Path) -> PathBuf {
    std::env::var_os("STRATEGY_API_PROFILE_DB")
        .filter(|path| !path.is_empty())
        .map(|path| root.join(path))
        .unwrap_or_else(|| root.join("data/api-profiles.sqlite"))
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct WorkspaceSettings {
    ai_provider: String,
    openai_model: String,
    openai_models: Vec<String>,
    openai_key: Option<String>,
    anthropic: AiSource,
    gemini: AiSource,
    grok: AiSource,
    mixroute: AiSource,
    telegram_chat_id: String,
    telegram_bot_token: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct AiSource {
    model: String,
    models: Vec<String>,
    key: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct AiPreferences {
    ai_provider: String,
    openai_models: Vec<String>,
    anthropic: AiSource,
    gemini: AiSource,
    grok: AiSource,
    mixroute: AiSource,
}

fn normalize_model(model: &mut String) -> Result<(), String> {
    *model = model.trim().into();
    if model.len() > 128 || model.contains(char::is_whitespace) {
        return Err("模型名稱不可含有空白，且最多 128 個位元組".into());
    }
    Ok(())
}

fn normalize_models(models: &mut Vec<String>) -> Result<(), String> {
    if models.len() > 32 {
        return Err("每個 AI 來源最多可設定 32 個模型".into());
    }
    for model in models.iter_mut() {
        normalize_model(model)?;
    }
    models.retain(|model| !model.is_empty());
    Ok(())
}

fn read_workspace_settings(db: &Connection) -> Result<WorkspaceSettings, String> {
    let mut settings: WorkspaceSettings = read_settings_record(db, "workspace_settings")?
        .unwrap_or_default();
    if let Some(ai) = read_settings_record::<AiPreferences>(db, "workspace_ai_settings")? {
        settings.ai_provider = ai.ai_provider;
        settings.openai_models = ai.openai_models;
        settings.anthropic = ai.anthropic;
        settings.gemini = ai.gemini;
        settings.grok = ai.grok;
        settings.mixroute = ai.mixroute;
    }
    Ok(settings)
}

fn read_settings_record<T: serde::de::DeserializeOwned>(
    db: &Connection, table: &str,
) -> Result<Option<T>, String> {
    let exists: bool = sql(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    ))?;
    if !exists {
        return Ok(None);
    }
    let value: Option<Vec<u8>> = sql(db
        .query_row(&format!("SELECT encrypted FROM {table} WHERE id=1"), [], |row| row.get(0))
        .optional())?;
    match value {
        Some(encrypted) => serde_json::from_slice(&protect(&encrypted, false)?).map(Some)
            .map_err(|_| "無法讀取工作台設定".into()),
        None => Ok(None),
    }
}

fn public_workspace_settings(settings: &WorkspaceSettings) -> Value {
    let source = |model: &str, models: &[String], key: &Option<String>| {
        json!({"model":model,"models":models,"key_set":key.is_some()})
    };
    json!({
        "ai_provider":if settings.ai_provider.is_empty() {"openai"} else {&settings.ai_provider},
        "ai_sources":{
            "openai":source(&settings.openai_model,&settings.openai_models,&settings.openai_key),
            "anthropic":source(&settings.anthropic.model,&settings.anthropic.models,&settings.anthropic.key),
            "gemini":source(&settings.gemini.model,&settings.gemini.models,&settings.gemini.key),
            "grok":source(&settings.grok.model,&settings.grok.models,&settings.grok.key),
            "mixroute":source(&settings.mixroute.model,&settings.mixroute.models,&settings.mixroute.key)
        },
        "telegram_chat_id":settings.telegram_chat_id,
        "telegram_bot_token_set":settings.telegram_bot_token.is_some()
    })
}

fn workspace_settings(root: &Path, action: &str, args: Value) -> Result<Value, String> {
    let path = location(root);
    if action == "save" {
        let mut next: WorkspaceSettings =
            serde_json::from_value(args.clone()).map_err(|_| "工作台設定欄位或型別無效")?;
        if args.get("ai_provider").is_some()
            && !["openai", "anthropic", "gemini", "grok", "mixroute"].contains(&next.ai_provider.as_str())
        {
            return Err("請選擇支援的 AI 來源".into());
        }
        normalize_model(&mut next.openai_model)?;
        normalize_models(&mut next.openai_models)?;
        for source in [&mut next.anthropic, &mut next.gemini, &mut next.grok, &mut next.mixroute] {
            normalize_model(&mut source.model)?;
            normalize_models(&mut source.models)?;
        }
        next.telegram_chat_id = next.telegram_chat_id.trim().into();
        let chat = &next.telegram_chat_id;
        if !chat.is_empty()
            && chat.parse::<i64>().ok().filter(|id| *id != 0).is_none()
            && !chat.strip_prefix('@').is_some_and(|name| {
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
        {
            return Err("Chat ID 請填入數字（群組可含負號）或 @頻道名稱".into());
        }
        for token in [&mut next.openai_key, &mut next.anthropic.key, &mut next.gemini.key, &mut next.grok.key, &mut next.mixroute.key, &mut next.telegram_bot_token].into_iter().flatten() {
            *token = token.trim().into();
            if token.contains(char::is_whitespace) || token.len() > 4096 {
                return Err("API 金鑰或 Bot Token 格式無效".into());
            }
        }
        let mut db = open(&path)?;
        let tx = sql(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        sql(tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS workspace_settings(
                id INTEGER PRIMARY KEY CHECK (id=1), encrypted BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS workspace_ai_settings(
                id INTEGER PRIMARY KEY CHECK (id=1), encrypted BLOB NOT NULL);",
        ))?;
        let previous = read_workspace_settings(&tx)?;
        if args.get("ai_provider").is_none() {
            next.ai_provider = previous.ai_provider;
        }
        if args.get("openai_model").is_none() {
            next.openai_model = previous.openai_model;
        }
        if args.get("openai_models").is_none() {
            next.openai_models = previous.openai_models;
        }
        for (name, current, prior) in [
            ("anthropic", &mut next.anthropic, previous.anthropic),
            ("gemini", &mut next.gemini, previous.gemini),
            ("grok", &mut next.grok, previous.grok),
            ("mixroute", &mut next.mixroute, previous.mixroute),
        ] {
            if args[name].get("model").is_none() { current.model = prior.model; }
            if args[name].get("models").is_none() { current.models = prior.models; }
            current.key = current.key.take().or(prior.key).filter(|v| !v.is_empty());
        }
        if args.get("telegram_chat_id").is_none() {
            next.telegram_chat_id = previous.telegram_chat_id;
        }
        next.openai_key = next.openai_key.or(previous.openai_key).filter(|v| !v.is_empty());
        next.telegram_bot_token = next.telegram_bot_token.or(previous.telegram_bot_token)
            .filter(|v| !v.is_empty());
        let basic = json!({
            "openai_model":next.openai_model,"openai_key":next.openai_key,
            "telegram_chat_id":next.telegram_chat_id,"telegram_bot_token":next.telegram_bot_token
        });
        let ai = json!({
            "ai_provider":next.ai_provider,"openai_models":next.openai_models,
            "anthropic":next.anthropic,"gemini":next.gemini,"grok":next.grok,"mixroute":next.mixroute
        });
        for (table, value) in [("workspace_settings", basic), ("workspace_ai_settings", ai)] {
            let cleartext = serde_json::to_vec(&value).map_err(|_| "無法編碼工作台設定")?;
            let encrypted = protect(&cleartext, true)?;
            sql(tx.execute(
                &format!("INSERT INTO {table}(id, encrypted) VALUES (1, ?1)
                          ON CONFLICT(id) DO UPDATE SET encrypted=excluded.encrypted"),
                [encrypted],
            ))?;
        }
        sql(tx.commit())?;
        return Ok(public_workspace_settings(&next));
    }
    let settings = if path.try_exists().map_err(|e| e.to_string())? {
        read_workspace_settings(&read_only(&path)?)?
    } else {
        WorkspaceSettings::default()
    };
    match action {
        "read" => Ok(public_workspace_settings(&settings)),
        "ai" => {
            let provider = args["provider"].as_str().filter(|v| !v.is_empty())
                .unwrap_or(if settings.ai_provider.is_empty() {"openai"} else {&settings.ai_provider});
            let (model, key) = match provider {
                "openai" => (settings.openai_model, settings.openai_key),
                "anthropic" => (settings.anthropic.model, settings.anthropic.key),
                "gemini" => (settings.gemini.model, settings.gemini.key),
                "grok" => (settings.grok.model, settings.grok.key),
                "mixroute" => (settings.mixroute.model, settings.mixroute.key),
                _ => return Err("無法辨識 AI 來源".into()),
            };
            Ok(json!({"provider":provider,"model":model,"key":key}))
        }
        "telegram" => Ok(json!({"chat_id":settings.telegram_chat_id, "token":settings.telegram_bot_token})),
        "reveal_telegram" => Ok(json!(settings.telegram_bot_token)),
        _ => Err("未知工作台設定操作".into()),
    }
}

fn open(path: &Path) -> Result<Connection, String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let c = sql(Connection::open(path))?;
    sql(c.busy_timeout(std::time::Duration::from_secs(5)))?;
    sql(c.execute_batch(
        "CREATE TABLE IF NOT EXISTS api_profiles(
        id TEXT PRIMARY KEY COLLATE NOCASE, profile TEXT NOT NULL, credentials BLOB);",
    ))?;
    Ok(c)
}

fn validate(profile: &Profile) -> Result<(), String> {
    if profile.id.is_empty()
        || profile.id.len() > 64
        || !profile
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("設定名稱須為 1 至 64 個英數字、連字號或底線".into());
    }
    if profile.provider.trim().is_empty() || !["quote", "trade"].contains(&profile.purpose.as_str())
    {
        return Err("請填寫供應商及用途".into());
    }
    if profile.port == Some(0) {
        return Err("連接埠須為 1 至 65535".into());
    }
    if !profile.ca_path.is_empty() && !Path::new(&profile.ca_path).is_absolute() {
        return Err("CA 憑證須填寫本機絕對路徑".into());
    }
    Ok(())
}

fn read_only(path: &Path) -> Result<Connection, String> {
    let c = sql(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    ))?;
    sql(c.busy_timeout(std::time::Duration::from_secs(5)))?;
    Ok(c)
}

fn decode(blob: Option<Vec<u8>>) -> Result<Credentials, String> {
    match blob {
        Some(blob) => {
            serde_json::from_slice(&protect(&blob, false)?).map_err(|_| "憑證內容無法讀取".into())
        }
        None => Ok(Credentials::default()),
    }
}

fn public(profile: Profile, blob: Option<Vec<u8>>) -> Value {
    let available = decode(blob);
    let readable = available.is_ok();
    let creds = available.unwrap_or_default();
    let secret_keys: Vec<_> = creds.secrets.keys().collect();
    json!({
        "profile":profile, "credentials_readable":readable,
        "account_set":creds.account.is_some(), "password_set":creds.password.is_some(),
        "token_set":creds.token.is_some(), "ca_password_set":creds.ca_password.is_some(),
        "secret_keys":secret_keys
    })
}

pub fn load(path: &Path, id: &str) -> Result<(Profile, Credentials), String> {
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Err("請先建立 API 連線設定".into());
    }
    let c = read_only(path)?;
    let (body, blob): (String, Option<Vec<u8>>) = sql(c
        .query_row(
            "SELECT profile,credentials FROM api_profiles WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional())?
    .ok_or("找不到 API 連線設定")?;
    Ok((
        serde_json::from_str(&body).map_err(|e| e.to_string())?,
        decode(blob)?,
    ))
}

pub fn call(root: &Path, action: &str, args: Value) -> Result<Value, String> {
    let path = location(root);
    match action {
        "api_profile_schema" => {
            let request: SchemaRequest = serde_json::from_value(args)
                .map_err(|_| "請提供供應商代碼及用途")?;
            if !["quote", "trade"].contains(&request.purpose.as_str()) {
                return Err("用途須為 quote 或 trade".into());
            }
            Ok(schema(&request.provider, &request.purpose))
        }
        "api_profile_list" => {
            if !path.try_exists().map_err(|e| e.to_string())? {
                return Ok(json!({"profiles":[]}));
            }
            let c = read_only(&path)?;
            let mut stmt =
                sql(c.prepare("SELECT profile,credentials FROM api_profiles ORDER BY id"))?;
            let mut rows = sql(stmt.query([]))?;
            let mut profiles = Vec::new();
            while let Some(r) = sql(rows.next())? {
                let body: String = sql(r.get(0))?;
                profiles.push(public(
                    serde_json::from_str(&body).map_err(|e| e.to_string())?,
                    sql(r.get(1))?,
                ));
            }
            Ok(json!({"profiles":profiles}))
        }
        "api_profile_save" => {
            let change: Change =
                serde_json::from_value(args).map_err(|_| "API 設定欄位或型別無效")?;
            validate(&change.profile)?;
            let incoming = change.credentials;
            if change.profile.options.keys().chain(incoming.secrets.keys())
                .any(|key| !valid_option_key(key))
            {
                return Err("自訂欄位名稱須為 1 至 64 個英數字、連字號、底線或點".into());
            }
            if change.profile.provider == "kgi_quotecom" && change.profile.purpose == "quote"
                && (!change.profile.options.is_empty() || !incoming.secrets.is_empty())
            {
                return Err("凱基 QuoteCom 行情請使用指定欄位".into());
            }
            let replaces_all = [
                &incoming.account,
                &incoming.password,
                &incoming.token,
                &incoming.ca_password,
            ]
            .iter()
            .all(|value| value.is_some());
            let mut c = open(&path)?;
            let tx = sql(c.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let previous: Option<(String, Option<Vec<u8>>)> = sql(tx
                .query_row(
                    "SELECT profile,credentials FROM api_profiles WHERE id=?1",
                    [&change.profile.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional())?;
            let mut creds = Credentials::default();
            if let Some((body, blob)) = previous {
                let old: Profile = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                if old.provider == change.profile.provider
                    && old.purpose == change.profile.purpose
                {
                    creds = decode(blob).or_else(|error| {
                        if replaces_all { Ok(Credentials::default()) } else { Err(error) }
                    })?;
                }
            }
            for (current, next) in [
                (&mut creds.account, incoming.account),
                (&mut creds.password, incoming.password),
                (&mut creds.token, incoming.token),
                (&mut creds.ca_password, incoming.ca_password),
            ] {
                if let Some(value) = next {
                    *current = (!value.is_empty()).then_some(value);
                }
            }
            for (key, value) in incoming.secrets {
                if value.is_empty() {
                    creds.secrets.remove(&key);
                } else {
                    creds.secrets.insert(key, value);
                }
            }
            if change.profile.options.keys().any(|key| creds.secrets.contains_key(key)) {
                return Err("自訂公開欄位與敏感欄位不可同名".into());
            }
            let blob = if !creds.secrets.is_empty() || [
                &creds.account,
                &creds.password,
                &creds.token,
                &creds.ca_password,
            ]
            .iter()
            .any(|v| v.is_some())
            {
                Some(protect(
                    &serde_json::to_vec(&creds).map_err(|_| "無法編碼憑證")?,
                    true,
                )?)
            } else {
                None
            };
            let body = serde_json::to_string(&change.profile).map_err(|e| e.to_string())?;
            sql(tx.execute("INSERT INTO api_profiles VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET profile=excluded.profile,credentials=excluded.credentials",
                params![change.profile.id,body,blob]))?;
            sql(tx.commit())?;
            Ok(public(change.profile, blob))
        }
        _ => Err("未知 API 設定操作".into()),
    }
}

#[cfg(windows)]
fn protect(input: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    #[repr(C)]
    struct Blob {
        length: u32,
        data: *mut u8,
    }
    #[link(name = "Crypt32")]
    extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *const std::ffi::c_void,
            prompt: *const std::ffi::c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *const std::ffi::c_void,
            prompt: *const std::ffi::c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn LocalFree(ptr: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    let mut bytes = input.to_vec();
    let source = Blob {
        length: u32::try_from(bytes.len()).map_err(|_| "資料過長")?,
        data: bytes.as_mut_ptr(),
    };
    let mut output = Blob {
        length: 0,
        data: std::ptr::null_mut(),
    };
    let null = std::ptr::null();
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &source,
                null,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        };
        bytes.fill(0);
        if ok == 0 {
            return Err(format!(
                "Windows 憑證保護失敗：{}",
                std::io::Error::last_os_error()
            ));
        }
        let decoded = std::slice::from_raw_parts(output.data, output.length as usize);
        let result = decoded.to_vec();
        LocalFree(output.data.cast());
        Ok(result)
    }
}

#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("此版本的憑證儲存使用 Windows DPAPI".into())
}

#[derive(GodotClass)]
#[class(base=RefCounted,init)]
struct ApiProfileBridge {
    base: Base<RefCounted>,
}

#[godot_api]
impl ApiProfileBridge {
    #[func]
    fn settings(&self, root: GString, action: GString, args: GString) -> GString {
        let result = serde_json::from_str(&args.to_string())
            .map_err(|_| "設定格式錯誤".to_string())
            .and_then(|v| workspace_settings(Path::new(&root.to_string()), &action.to_string(), v));
        let response = match result {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        response.to_string().as_str().into()
    }

    #[func]
    fn reveal(&self, root: GString, id: GString, key: GString, custom: bool) -> GString {
        let result = load(&location(Path::new(&root.to_string())), &id.to_string()).map(
            |(_, credentials)| {
                let key = key.to_string();
                if custom {
                    return credentials.secrets.get(&key).cloned();
                }
                match key.as_str() {
                    "account" => credentials.account,
                    "password" => credentials.password,
                    "token" => credentials.token,
                    "ca_password" => credentials.ca_password,
                    _ => None,
                }
            },
        );
        let response = match result {
            Ok(Some(value)) => json!({"ok":true,"data":value}),
            Ok(None) => json!({"ok":false,"error":"這個欄位目前沒有儲存值。"}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        response.to_string().as_str().into()
    }

    #[func]
    fn request(&self, root: GString, action: GString, args: GString) -> GString {
        let result = serde_json::from_str(&args.to_string())
            .map_err(|_| "設定格式錯誤".to_string())
            .and_then(|v| call(Path::new(&root.to_string()), &action.to_string(), v));
        let response = match result {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        response.to_string().as_str().into()
    }
}
