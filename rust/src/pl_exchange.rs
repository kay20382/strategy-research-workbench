//! PowerLanguage 文字交換，內部仍使用 Unicode。
use std::{fs, path::Path};

pub fn read(path: &Path) -> Result<String, String> {
    let raw = fs::read(path).map_err(|error| error.to_string())?;
    if let Ok(source) = std::str::from_utf8(&raw) {
        return Ok(source.trim_start_matches('\u{feff}').to_owned());
    }
    #[cfg(windows)]
    return crate::market_csv::big5(&raw)
        .map_err(|_| "原始碼不是有效的 UTF-8 或 Big5／CP950".into());
    #[cfg(not(windows))]
    Err("目前只在 Windows 支援 Big5／CP950 策略原始碼".into())
}

#[cfg(windows)]
pub fn encode(source: &str) -> Result<Vec<u8>, String> {
    use std::ptr::{null, null_mut};
    #[link(name = "kernel32")]
    extern "system" {
        fn WideCharToMultiByte(
            page: u32,
            flags: u32,
            source: *const u16,
            length: i32,
            output: *mut u8,
            capacity: i32,
            default: *const u8,
            substituted: *mut i32,
        ) -> i32;
    }
    let source = source
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    if source.is_empty() {
        return Ok(Vec::new());
    }
    let wide: Vec<_> = source.encode_utf16().collect();
    let len = i32::try_from(wide.len()).map_err(|_| "PowerLanguage 原始碼過長")?;
    let mut substituted = 0;
    // WC_NO_BEST_FIT_CHARS avoids replacing similar-looking characters.
    let size = unsafe {
        WideCharToMultiByte(
            950,
            0x400,
            wide.as_ptr(),
            len,
            null_mut(),
            0,
            null(),
            &mut substituted,
        )
    };
    if size <= 0 || substituted != 0 {
        return Err("原始碼含 CP950 無法表示的字元，請改用可編碼的繁體中文文字".into());
    }
    let mut bytes = vec![0; size as usize];
    let written = unsafe {
        WideCharToMultiByte(
            950,
            0x400,
            wide.as_ptr(),
            len,
            bytes.as_mut_ptr(),
            size,
            null(),
            &mut substituted,
        )
    };
    if written != size
        || substituted != 0
        || crate::market_csv::big5(&bytes).as_deref() != Ok(source.as_str())
    {
        return Err("原始碼無法無損轉成 Big5／CP950".into());
    }
    Ok(bytes)
}

#[cfg(not(windows))]
pub fn encode(_source: &str) -> Result<Vec<u8>, String> {
    Err("目前只在 Windows 支援 Big5／CP950 匯出".into())
}

pub fn export(root: &Path, source: &str) -> Result<serde_json::Value, String> {
    let bytes = encode(source)?;
    let parent = root.join("outputs/powerlanguage");
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let dir = parent.join(crate::strategy_library::fresh_id());
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("strategy.pl");
    fs::write(&file, bytes).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"path":file,"encoding":"windows-950","line_endings":"CRLF"}))
}

pub(crate) fn validate_generated(source: &str) -> Result<(), String> {
    let has_chinese = |text: &str| text.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    let mut rest = source;
    while let Some(character) = rest.chars().next() {
        if let Some(comment) = rest.strip_prefix("//") {
            let end = comment.find(['\r', '\n']).unwrap_or(comment.len());
            if has_chinese(&comment[..end]) {
                return encode(source).map(|_| ());
            }
            rest = &comment[end..];
        } else if let Some(comment) = rest.strip_prefix('{') {
            let end = comment.find('}').ok_or("PowerLanguage 區塊註釋未結束")?;
            if has_chinese(&comment[..end]) {
                return encode(source).map(|_| ());
            }
            rest = &comment[end + 1..];
        } else if let Some(literal) = rest.strip_prefix('"') {
            let end = literal.find('"').ok_or("PowerLanguage 字串未結束")?;
            rest = &literal[end + 1..];
        } else {
            rest = &rest[character.len_utf8()..];
        }
    }
    Err("AI 產生的 PowerLanguage 須包含繁體中文註釋，說明關鍵交易規則".into())
}
