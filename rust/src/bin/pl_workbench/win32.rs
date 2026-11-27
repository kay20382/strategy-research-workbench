//! 僅此模組處理 Win32 ABI；交易、行情及報表流程位於安全 Rust 模組。
use recharts_core::strategy_library::{self as library, Document, Entry};
use recharts_core::workbench as core;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    ffi::c_void,
    fs,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::null,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
type H = isize;
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
struct Msg {
    hwnd: H,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}
#[repr(C)]
struct Class {
    style: u32,
    proc: unsafe extern "system" fn(H, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: H,
    icon: H,
    cursor: H,
    background: H,
    menu: *const u16,
    name: *const u16,
}
#[repr(C)]
struct OpenFile {
    size: u32,
    owner: H,
    instance: H,
    filter: *const u16,
    custom: *mut u16,
    max_custom: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_title: u32,
    initial: *const u16,
    title: *const u16,
    flags: u32,
    offset: u16,
    extension: u16,
    default_ext: *const u16,
    custom_data: isize,
    hook: usize,
    template: *const u16,
    reserved: *mut c_void,
    reserved2: u32,
    flags_ex: u32,
}
#[repr(C)]
struct TabItem {
    mask: u32,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_max: i32,
    image: i32,
    param: isize,
}
#[repr(C)]
struct Notify {
    window: H,
    id: usize,
    code: u32,
}
#[repr(C)]
struct CommonControls {
    size: u32,
    classes: u32,
}
#[repr(C)]
struct Range {
    start: i32,
    end: i32,
}
#[repr(C)]
struct CharFormat {
    size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    color: u32,
    charset: u8,
    pitch: u8,
    face: [u16; 32],
}
#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(c: *const Class) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: H,
        menu: H,
        instance: H,
        param: *const c_void,
    ) -> H;
    fn DefWindowProcW(h: H, m: u32, w: usize, l: isize) -> isize;
    fn GetMessageW(m: *mut Msg, h: H, min: u32, max: u32) -> i32;
    fn TranslateMessage(m: *const Msg) -> i32;
    fn DispatchMessageW(m: *const Msg) -> isize;
    fn IsDialogMessageW(h: H, m: *const Msg) -> i32;
    fn ShowWindow(h: H, n: i32) -> i32;
    fn UpdateWindow(h: H) -> i32;
    fn GetClientRect(h: H, r: *mut Rect) -> i32;
    fn MoveWindow(h: H, x: i32, y: i32, w: i32, height: i32, repaint: i32) -> i32;
    fn SetWindowTextW(h: H, s: *const u16) -> i32;
    fn GetWindowTextLengthW(h: H) -> i32;
    fn GetWindowTextW(h: H, s: *mut u16, n: i32) -> i32;
    fn SendMessageW(h: H, m: u32, w: usize, l: isize) -> isize;
    fn EnableWindow(h: H, b: i32) -> i32;
    fn SetFocus(h: H) -> H;
    fn GetFocus() -> H;
    fn GetKeyState(key: i32) -> i16;
    fn LoadCursorW(instance: H, name: *const u16) -> H;
    fn MessageBoxW(h: H, text: *const u16, title: *const u16, kind: u32) -> i32;
    fn SetTimer(h: H, id: usize, ms: u32, callback: usize) -> usize;
    fn PostQuitMessage(n: i32);
    fn DestroyWindow(h: H) -> i32;
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    fn CreateMenu() -> H;
    fn CreatePopupMenu() -> H;
    fn AppendMenuW(menu: H, flags: u32, id: usize, title: *const u16) -> i32;
    fn SetMenu(window: H, menu: H) -> i32;
    fn InvalidateRect(window: H, rect: *const Rect, erase: i32) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(s: *const u16) -> H;
    fn LoadLibraryW(s: *const u16) -> H;
}
#[link(name = "comctl32")]
extern "system" {
    fn InitCommonControlsEx(c: *const CommonControls) -> i32;
}
#[link(name = "gdi32")]
extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        esc: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> H;
    fn DeleteObject(h: H) -> i32;
}
#[link(name = "comdlg32")]
extern "system" {
    fn GetOpenFileNameW(f: *mut OpenFile) -> i32;
    fn GetSaveFileNameW(f: *mut OpenFile) -> i32;
}
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        h: H,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> isize;
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn set(h: H, s: &str) {
    SetWindowTextW(
        h,
        wide(&s.replace("\r\n", "\n").replace('\n', "\r\n")).as_ptr(),
    );
}
unsafe fn text(h: H) -> String {
    let len = GetWindowTextLengthW(h);
    let mut b = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
    String::from_utf16_lossy(&b[..n as usize])
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}
unsafe fn alert(h: H, s: &str) {
    MessageBoxW(h, wide(s).as_ptr(), wide("PL 回測工作台").as_ptr(), 0x40);
}
unsafe fn position(h: H, x: i32, y: i32, w: i32, ht: i32) {
    MoveWindow(h, x, y, w.max(10), ht.max(10), 1);
}
unsafe fn child(parent: H, class: &str, title: &str, style: u32, id: usize, font: H) -> H {
    let h = CreateWindowExW(
        if class == "EDIT" || class == "RICHEDIT50W" {
            0x200
        } else {
            0
        },
        wide(class).as_ptr(),
        wide(title).as_ptr(),
        0x50000000 | style,
        0,
        0,
        100,
        28,
        parent,
        id as H,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(h, 0x30, font as usize, 1);
    if class == "EDIT" {
        SendMessageW(h, 0xc5, 2_000_000, 0);
    }
    if class == "RICHEDIT50W" {
        SendMessageW(h, 0x435, 0, 2_000_000);
        SendMessageW(h, 0x445, 0, 1 | 4);
    }
    h
}
struct Field {
    key: &'static str,
    label: H,
    edit: H,
}
enum Event {
    Log(String),
    Done(Result<Value, String>),
    Imported(Result<Value, String>),
}
struct Ui {
    window: H,
    font: H,
    mono: H,
    toolbar: Vec<H>,
    tabs: Vec<H>,
    notice: H,
    code_hint: H,
    code: H,
    fields: Vec<Field>,
    inputs_label: H,
    inputs: H,
    advanced_label: H,
    advanced: H,
    datasets: H,
    logs: H,
    status: H,
    excel: H,
    directory_label: H,
    directory: H,
    open_buttons: Vec<H>,
    page: usize,
    root: PathBuf,
    result: Option<Value>,
    receiver: Option<Receiver<Event>>,
    busy: bool,
    data_fields: Vec<Field>,
    data_hint: H,
    data_list: H,
    data_buttons: Vec<H>,
    navigator_label: H,
    search: H,
    library_kind: H,
    library_list: H,
    editor_tabs: H,
    line_numbers: H,
    output_label: H,
    result_summary: H,
    entries: Vec<Entry>,
    visible_entries: Vec<usize>,
    documents: Vec<Document>,
    active: usize,
    changed_at: Option<Instant>,
    session_pending: bool,
    last_session_save: Instant,
}
thread_local! {static UI:RefCell<Option<Ui>>=const{RefCell::new(None)};}
impl Ui {
    unsafe fn new(window: H) -> Result<Self, String> {
        let font = CreateFontW(
            -18,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            wide("Microsoft JhengHei UI").as_ptr(),
        );
        let mono = CreateFontW(
            -18,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            wide("Consolas").as_ptr(),
        );
        let toolbar = [
            ("新增策略", 140),
            ("匯入 PL", 102),
            ("儲存策略", 103),
            ("匯出 PL", 143),
            ("檢查語法", 144),
            ("開始回測", 104),
            ("關閉分頁", 142),
        ]
        .iter()
        .map(|(t, id)| child(window, "BUTTON", t, 0x10000, *id, font))
        .collect();
        let tabs = ["PL 程式碼", "回測設定", "執行與報表", "行情資料／匯入"]
            .iter()
            .enumerate()
            .map(|(i, t)| child(window, "BUTTON", t, 0x10000, 110 + i, font))
            .collect();
        let notice = child(window, "STATIC", core::LIMITATION, 0, 0, font);
        let code_hint = child(
            window,
            "STATIC",
            "匯入 .pl 檔，或點選下方編輯區直接貼上 PL 程式碼，再按「開始回測」。",
            0,
            0,
            font,
        );
        let code = child(
            window,
            "RICHEDIT50W",
            "",
            0x300000 | 0x10000 | 0x4 | 0x1000 | 0x40 | 0x80,
            201,
            mono,
        );
        let mut fields = vec![];
        for (i, (key, label)) in [
            ("strategy_name", "策略名稱"),
            ("symbol", "商品代碼"),
            ("from", "開始時間"),
            ("to", "結束時間"),
            ("bar_unit", "K 棒類型"),
            ("bar_period", "K 棒週期數量"),
            ("warmup_bars", "MaxBarsBack／暖機棒數"),
            ("initial_capital", "初始資本"),
            ("contracts", "交易口數"),
            ("commission_per_side", "單邊每口成本"),
            ("slippage_ticks", "單邊滑價（跳）"),
        ]
        .iter()
        .enumerate()
        {
            fields.push(Field {
                key,
                label: child(window, "STATIC", label, 0, 0, font),
                edit: if *key == "bar_unit" {
                    let h = child(
                        window,
                        "COMBOBOX",
                        "",
                        0x10000 | 0x200000 | 3,
                        210 + i,
                        font,
                    );
                    for s in ["秒 K", "分 K", "日 K"] {
                        SendMessageW(h, 0x143, 0, wide(s).as_ptr() as isize);
                    }
                    h
                } else {
                    child(window, "EDIT", "", 0x10000 | 0x80, 210 + i, font)
                },
            });
        }
        let inputs_label = child(
            window,
            "STATIC",
            "策略輸入值（JSON；{} 使用 PL 預設值）",
            0,
            0,
            font,
        );
        let inputs = child(
            window,
            "EDIT",
            "{}",
            0x200000 | 0x10000 | 4 | 0x1000 | 0x40,
            230,
            mono,
        );
        let advanced_label = child(
            window,
            "STATIC",
            "完整任務設定（JSON）；上方欄位於執行時覆蓋對應值",
            0,
            0,
            font,
        );
        let advanced = child(
            window,
            "EDIT",
            "",
            0x300000 | 0x10000 | 4 | 0x1000 | 0x40 | 0x80,
            231,
            mono,
        );
        let datasets = child(window, "BUTTON", "檢視行情資料集", 0x10000, 105, font);
        let logs = child(
            window,
            "EDIT",
            "",
            0x300000 | 0x10000 | 4 | 0x800 | 0x40 | 0x80,
            240,
            mono,
        );
        let status = child(window, "STATIC", "就緒｜Rust 原生 Windows 介面", 0, 0, font);
        let excel = child(
            window,
            "BUTTON",
            "產出 Excel 並驗證",
            0x10000 | 3,
            250,
            font,
        );
        SendMessageW(excel, 0xf1, 1, 0);
        let directory_label = child(window, "STATIC", "報告資料夾", 0, 0, font);
        let directory = child(window, "EDIT", "", 0x10000 | 0x80, 251, font);
        let open_buttons = [
            "開啟 HTML 報表",
            "開啟 Excel",
            "開啟交易明細",
            "開啟本次資料夾",
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| child(window, "BUTTON", t, 0x10000, 120 + i, font))
        .collect();
        let root = core::root();
        let navigator_label = child(window, "STATIC", "策略管理", 0, 0, font);
        let search = child(window, "EDIT", "", 0x10000 | 0x80, 401, font);
        SendMessageW(search, 0x1501, 0, wide("搜尋策略或函式").as_ptr() as isize);
        let library_kind = child(window, "COMBOBOX", "", 0x10000 | 0x200000 | 3, 402, font);
        for s in ["策略", "函式"] {
            SendMessageW(library_kind, 0x143, 0, wide(s).as_ptr() as isize);
        }
        SendMessageW(library_kind, 0x14e, 0, 0);
        let library_list = child(
            window,
            "LISTBOX",
            "",
            0x10000 | 0x200000 | 0x100000 | 1 | 0x100,
            403,
            font,
        );
        let editor_tabs = child(window, "SysTabControl32", "", 0x10000, 404, font);
        let line_numbers = child(window, "EDIT", "1", 4 | 0x800 | 2, 405, mono);
        let output_label = child(window, "STATIC", "檢查／回測輸出", 0, 0, font);
        let result_summary = child(
            window,
            "EDIT",
            "尚未執行回測。",
            4 | 0x800 | 0x40,
            406,
            font,
        );
        let mut data_fields = vec![];
        for (i, (key, label)) in [
            ("symbol", "商品代碼"),
            ("kind", "基礎資料格式"),
            ("timezone", "時區（未知填 unspecified）"),
            ("role", "時間標記"),
            ("file", "匯入檔案路徑（CSV／TXT）"),
        ]
        .iter()
        .enumerate()
        {
            let edit = if *key == "kind" || *key == "role" {
                let h = child(
                    window,
                    "COMBOBOX",
                    "",
                    0x10000 | 0x200000 | 3,
                    300 + i,
                    font,
                );
                let choices = if *key == "kind" {
                    vec!["Tick", "1 分 K", "日 K"]
                } else {
                    vec!["event", "end", "start", "unspecified"]
                };
                for s in choices {
                    SendMessageW(h, 0x143, 0, wide(s).as_ptr() as isize);
                }
                SendMessageW(h, 0x14e, 0, 0);
                h
            } else {
                child(window, "EDIT", "", 0x10000 | 0x80, 300 + i, font)
            };
            data_fields.push(Field {
                key,
                label: child(window, "STATIC", label, 0, 0, font),
                edit,
            });
        }
        set(data_fields[0].edit, "TXF1");
        set(data_fields[2].edit, "Asia/Taipei");
        let data_hint = child(
            window,
            "STATIC",
            "每商品僅 Tick／1分K／日K。Tick 以新檔起訖時間整段覆蓋；K棒同時間後蓋前。",
            0,
            0,
            font,
        );
        let data_list = child(
            window,
            "EDIT",
            "",
            0x300000 | 0x10000 | 4 | 0x800 | 0x40 | 0x80,
            310,
            mono,
        );
        let data_buttons = [
            ("選擇資料檔", 130),
            ("匯入資料", 131),
            ("重新整理資料來源", 132),
            ("商品／資料庫管理", 134),
        ]
        .iter()
        .map(|(s, id)| child(window, "BUTTON", s, 0x10000, *id, font))
        .collect();
        let pref = core::read_json(&root.join("data/report-output-settings.json"))?;
        set(directory, pref["directory"].as_str().unwrap_or(""));
        let mut ui = Self {
            window,
            font,
            mono,
            toolbar,
            tabs,
            notice,
            code_hint,
            code,
            fields,
            inputs_label,
            inputs,
            advanced_label,
            advanced,
            datasets,
            logs,
            status,
            excel,
            directory_label,
            directory,
            open_buttons,
            page: 0,
            root,
            result: None,
            receiver: None,
            busy: false,
            data_fields,
            data_hint,
            data_list,
            data_buttons,
            navigator_label,
            search,
            library_kind,
            library_list,
            editor_tabs,
            line_numbers,
            output_label,
            result_summary,
            entries: vec![],
            visible_entries: vec![],
            documents: vec![],
            active: 0,
            changed_at: None,
            session_pending: false,
            last_session_save: Instant::now(),
        };
        ui.load(&ui.root.join("tasks/pl-workbench-example.json"))?;
        if let Some((documents, active)) = library::restore_session(&ui.root)? {
            ui.documents = documents;
            ui.active = active;
            ui.display_document()?;
        } else {
            let task = ui.collect()?;
            ui.documents.push(Document {
                id: "initial-example".into(),
                name: task["parameters"]["strategy_name"]
                    .as_str()
                    .unwrap_or("範例")
                    .into(),
                kind: "strategy".into(),
                source: text(ui.code),
                task,
                origin: Some(ui.root.join("strategies/pl-workbench-example.pl")),
                managed_id: None,
                dirty: false,
                result: None,
            });
        }
        ui.entries = library::catalog(&ui.root)?;
        ui.refresh_library();
        ui.refresh_editor_tabs();
        ui.highlight();
        ui.refresh_sources()?;
        ui.show_page(0);
        Ok(ui)
    }
    unsafe fn data_database(&self) -> Result<PathBuf, String> {
        let task: Value = serde_json::from_str(&text(self.advanced)).map_err(|e| e.to_string())?;
        Ok(core::absolute(
            &self.root,
            task["database"].as_str().ok_or("缺少資料庫路徑")?,
        ))
    }
    unsafe fn refresh_sources(&self) -> Result<(), String> {
        let db = self.data_database()?;
        recharts_core::canonical_market::initialize(&self.root, &db)?;
        let rows = recharts_core::canonical_market::sources(&db)?;
        let list = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                format!(
                    "{}｜{}｜{} 筆｜來源 ID {}\n{} ～ {}\n時區 {}；時間標記 {}",
                    r["symbol"].as_str().unwrap_or(""),
                    match r["kind"].as_str().unwrap_or("") {
                        "tick" => "Tick",
                        "minute" => "1 分 K",
                        _ => "日 K",
                    },
                    r["row_count"],
                    r["dataset_id"],
                    r["first_datetime"].as_str().unwrap_or(""),
                    r["last_datetime"].as_str().unwrap_or(""),
                    r["timezone"].as_str().unwrap_or(""),
                    r["timestamp_role"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        set(
            self.data_list,
            if list.is_empty() {
                "目前沒有基礎行情，請先匯入。"
            } else {
                &list
            },
        );
        Ok(())
    }
    unsafe fn sync_import_metadata(&self) -> Result<(), String> {
        let db = self.data_database()?;
        let rows = recharts_core::canonical_market::sources(&db)?;
        let symbol = text(self.data_fields[0].edit);
        let kind = match text(self.data_fields[1].edit).as_str() {
            "Tick" => "tick",
            "1 分 K" => "minute",
            _ => "daily",
        };
        if let Some(row) = rows.as_array().unwrap().iter().find(|r| {
            r["symbol"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(symbol.trim()))
                && r["kind"] == kind
        }) {
            set(
                self.data_fields[2].edit,
                row["timezone"].as_str().unwrap_or("unspecified"),
            );
            SendMessageW(
                self.data_fields[3].edit,
                0x14e,
                match row["timestamp_role"].as_str().unwrap_or("") {
                    "event" => 0,
                    "end" => 1,
                    "start" => 2,
                    _ => 3,
                },
                0,
            );
        } else {
            SendMessageW(
                self.data_fields[3].edit,
                0x14e,
                if kind == "tick" { 0 } else { 1 },
                0,
            );
        }
        Ok(())
    }
    unsafe fn import_data(&mut self) -> Result<(), String> {
        if self.busy {
            return Ok(());
        }
        let database = self.data_database()?;
        let symbol = text(self.data_fields[0].edit).trim().to_owned();
        let kind = match text(self.data_fields[1].edit).as_str() {
            "Tick" => "tick",
            "1 分 K" => "minute",
            _ => "daily",
        }
        .to_owned();
        let zone = text(self.data_fields[2].edit).trim().to_owned();
        let role = text(self.data_fields[3].edit);
        let path = PathBuf::from(text(self.data_fields[4].edit).trim());
        if !path.is_file() {
            return Err("請先選擇有效的資料檔".into());
        }
        self.lock(true);
        set(self.status, "行情匯入中；整批驗證成功後才提交覆蓋…");
        let (send, recv) = mpsc::channel();
        self.receiver = Some(recv);
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                recharts_core::canonical_market::import(
                    &database, &path, &symbol, &kind, &zone, &role,
                )
            })
            .unwrap_or_else(|_| Err("匯入工作執行緒發生錯誤".into()));
            let _ = send.send(Event::Imported(result));
        });
        Ok(())
    }
    unsafe fn load(&mut self, path: &Path) -> Result<(), String> {
        let task = core::load_task(&self.root, path)?;
        let source = fs::read_to_string(task["strategy"].as_str().ok_or("設定缺少 PL 來源")?)
            .map_err(|e| e.to_string())?;
        self.display_task(&task, source.trim_start_matches('\u{feff}'))
    }
    unsafe fn display_task(&mut self, task: &Value, source: &str) -> Result<(), String> {
        let legacy_symbol = if task["market"]["symbol"].is_null() {
            core::datasets(&self.root, task["database"].as_str().ok_or("缺少資料庫")?)?
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == task["parameters"]["dataset_id"])
                .and_then(|r| r["symbol"].as_str())
                .unwrap_or("TXF1")
                .to_owned()
        } else {
            String::new()
        };
        let unit = task["market"]["unit"].as_str().unwrap_or_else(|| {
            if task["parameters"]["interval_minutes"] == 0 {
                "days"
            } else if task["report_settings"]["interval_label"]
                .as_str()
                .unwrap_or("")
                .contains("秒")
            {
                "seconds"
            } else {
                "minutes"
            }
        });
        let period = task["market"]["period"]
            .as_u64()
            .unwrap_or_else(|| match unit {
                "days" => 1,
                "seconds" => task["parameters"]["interval_minutes"].as_u64().unwrap_or(1) * 60,
                _ => task["parameters"]["interval_minutes"].as_u64().unwrap_or(1),
            });
        for f in &self.fields {
            if f.key == "symbol" {
                set(
                    f.edit,
                    task["market"]["symbol"].as_str().unwrap_or(&legacy_symbol),
                );
                continue;
            }
            if f.key == "bar_unit" {
                SendMessageW(
                    f.edit,
                    0x14e,
                    match unit {
                        "seconds" => 0,
                        "minutes" => 1,
                        _ => 2,
                    },
                    0,
                );
                continue;
            }
            if f.key == "bar_period" {
                set(f.edit, &period.to_string());
                continue;
            }
            let v = &task["parameters"][f.key];
            set(
                f.edit,
                if v.is_null() {
                    "".into()
                } else if let Some(s) = v.as_str() {
                    s.to_owned()
                } else {
                    v.to_string()
                }
                .as_str(),
            );
        }
        set(
            self.inputs,
            &serde_json::to_string_pretty(&task["parameters"]["inputs"])
                .map_err(|e| e.to_string())?,
        );
        set(
            self.advanced,
            &serde_json::to_string_pretty(&task).map_err(|e| e.to_string())?,
        );
        set(self.code, source);
        Ok(())
    }
    unsafe fn checkpoint(&mut self) -> Result<(), String> {
        if self.documents.is_empty() {
            return Ok(());
        }
        let task = self.collect()?;
        let source = text(self.code);
        let doc = &mut self.documents[self.active];
        if doc.task != task || doc.source != source {
            doc.dirty = true;
            self.result = None;
            self.session_pending = true;
        }
        doc.source = source;
        doc.task = task;
        doc.result = self.result.clone();
        if doc.kind == "strategy" {
            doc.name = doc.task["parameters"]["strategy_name"]
                .as_str()
                .unwrap_or(&doc.name)
                .to_owned();
        }
        Ok(())
    }
    unsafe fn display_document(&mut self) -> Result<(), String> {
        let doc = self.documents[self.active].clone();
        self.display_task(&doc.task, &doc.source)?;
        self.result = doc.result;
        set(
            self.status,
            &format!(
                "{}｜{}｜{}",
                doc.name,
                if doc.kind == "function" {
                    "函式"
                } else {
                    "策略"
                },
                if doc.dirty {
                    "尚未儲存"
                } else {
                    "已載入"
                }
            ),
        );
        self.update_result_summary();
        self.highlight();
        Ok(())
    }
    unsafe fn switch_document(&mut self, index: usize) -> Result<(), String> {
        if self.busy {
            return Err("回測或匯入執行中，請等待完成後再切換策略".into());
        }
        if index >= self.documents.len() {
            return Ok(());
        }
        self.checkpoint()?;
        self.active = index;
        self.display_document()?;
        self.refresh_editor_tabs();
        self.show_page(0);
        self.session_pending = true;
        Ok(())
    }
    unsafe fn refresh_editor_tabs(&self) {
        SendMessageW(self.editor_tabs, 0x1309, 0, 0);
        for (i, doc) in self.documents.iter().enumerate() {
            let mut name = wide(&format!(
                "{}{}",
                doc.name,
                if doc.dirty { " *" } else { "" }
            ));
            let mut item = TabItem {
                mask: 1,
                state: 0,
                state_mask: 0,
                text: name.as_mut_ptr(),
                text_max: 0,
                image: 0,
                param: 0,
            };
            SendMessageW(self.editor_tabs, 0x133e, i, &mut item as *mut _ as isize);
        }
        SendMessageW(self.editor_tabs, 0x130c, self.active, 0);
    }
    unsafe fn refresh_library(&mut self) {
        let filter = text(self.search).to_lowercase();
        let kind = if SendMessageW(self.library_kind, 0x147, 0, 0) == 1 {
            "function"
        } else {
            "strategy"
        };
        self.visible_entries = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == kind && e.name.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        SendMessageW(self.library_list, 0x184, 0, 0);
        for index in &self.visible_entries {
            let e = &self.entries[*index];
            let title = format!(
                "{}{}",
                e.name,
                if e.managed { "  [我的策略庫]" } else { "" }
            );
            SendMessageW(self.library_list, 0x180, 0, wide(&title).as_ptr() as isize);
        }
    }
    unsafe fn open_selected(&mut self) -> Result<(), String> {
        if self.busy {
            return Ok(());
        }
        let selected = SendMessageW(self.library_list, 0x188, 0, 0);
        if selected < 0 {
            return Ok(());
        }
        let Some(index) = self.visible_entries.get(selected as usize) else {
            return Ok(());
        };
        let entry = self.entries[*index].clone();
        if let Some(at) = self.documents.iter().position(|d| {
            d.id == entry.id
                || d.managed_id.as_deref() == Some(&entry.id)
                || d.origin.as_ref() == Some(&entry.source)
        }) {
            return self.switch_document(at);
        }
        self.checkpoint()?;
        let doc = library::open(&self.root, &entry)?;
        self.documents.push(doc);
        self.active = self.documents.len() - 1;
        self.display_document()?;
        self.refresh_editor_tabs();
        self.show_page(0);
        self.session_pending = true;
        Ok(())
    }
    unsafe fn save_document(&mut self) -> Result<(), String> {
        self.checkpoint()?;
        let entry = library::save(&self.root, &mut self.documents[self.active])?;
        self.entries = library::catalog(&self.root)?;
        self.refresh_library();
        self.refresh_editor_tabs();
        self.append(&format!("已儲存 {}；原始來源保留。", entry.name));
        set(self.status, &format!("{}｜已儲存至策略庫", entry.name));
        self.session_pending = true;
        Ok(())
    }
    unsafe fn persist_session(&mut self) -> Result<(), String> {
        self.checkpoint()?;
        library::save_session(&self.root, &self.documents, self.active)?;
        self.session_pending = false;
        self.last_session_save = Instant::now();
        Ok(())
    }
    unsafe fn code_changed(&mut self) {
        if let Some(doc) = self.documents.get_mut(self.active) {
            let source = text(self.code);
            if doc.source != source {
                doc.result = None;
                self.result = None;
            }
            doc.source = source;
            doc.dirty = true;
            self.changed_at = Some(Instant::now());
            self.session_pending = true;
            self.refresh_editor_tabs();
        }
    }
    unsafe fn update_result_summary(&self) {
        let Some(r) = self.result.as_ref() else {
            set(
                self.result_summary,
                "尚未執行回測。請選擇策略，確認設定後按「開始回測」。",
            );
            return;
        };
        if r["status"] != "completed" {
            set(
                self.result_summary,
                &format!(
                    "回測未完成\n{}",
                    r["error"].as_str().unwrap_or("請查看下方輸出紀錄")
                ),
            );
            return;
        }
        let s = &r["summary"];
        set(self.result_summary,&format!("策略：{}\n\n已平倉交易：{} 筆\n已平倉淨利：{:.2}\n勝率：{:.2}%\n最大策略回撤：{:.2}\n未平倉損益：{:.2}\n\n請使用上方按鈕開啟完整回測報表。",self.documents.get(self.active).map(|d|d.name.as_str()).unwrap_or(""),s["trades"],s["net_closed"].as_f64().unwrap_or(0.),s["win_rate"].as_f64().unwrap_or(0.),s["max_drawdown"].as_f64().unwrap_or(0.),s["open_pnl"].as_f64().unwrap_or(0.)));
    }
    unsafe fn highlight(&self) {
        let source = text(self.code);
        let mut selection = Range { start: 0, end: 0 };
        SendMessageW(self.code, 0x434, 0, &mut selection as *mut _ as isize);
        SendMessageW(self.code, 0xb, 0, 0);
        SendMessageW(self.code, 0x445, 0, 0);
        let paint = |start: i32, end: i32, color: u32| {
            let range = Range { start, end };
            SendMessageW(self.code, 0x437, 0, &range as *const _ as isize);
            let fmt = CharFormat {
                size: size_of::<CharFormat>() as u32,
                mask: 0x40000000,
                effects: 0,
                height: 0,
                offset: 0,
                color,
                charset: 0,
                pitch: 0,
                face: [0; 32],
            };
            SendMessageW(self.code, 0x444, 1, &fmt as *const _ as isize);
        };
        paint(0, -1, 0x202020);
        let chars: Vec<char> = source.chars().collect();
        let mut i = 0;
        let mut position = 0i32;
        while i < chars.len() {
            let start = i;
            let at = position;
            let mut color = None;
            if chars[i] == '{' {
                i += 1;
                while i < chars.len() && chars[i] != '}' {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
                color = Some(0x508020);
            } else if chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                color = Some(0x508020);
            } else if chars[i] == '"' {
                i += 1;
                while i < chars.len() {
                    let c = chars[i];
                    i += 1;
                    if c == '"' {
                        break;
                    }
                }
                color = Some(0x3050a0);
            } else if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word = chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase();
                if [
                    "if",
                    "then",
                    "else",
                    "begin",
                    "end",
                    "inputs",
                    "input",
                    "vars",
                    "var",
                    "variables",
                    "for",
                    "to",
                    "downto",
                    "and",
                    "or",
                    "not",
                    "next",
                    "this",
                    "bar",
                    "at",
                    "on",
                    "market",
                    "stop",
                    "limit",
                    "buy",
                    "sell",
                    "sellshort",
                    "buytocover",
                    "true",
                    "false",
                ]
                .contains(&word.as_str())
                {
                    color = Some(0xcc4010);
                } else if [
                    "rsi",
                    "average",
                    "xaverage",
                    "highest",
                    "lowest",
                    "close",
                    "high",
                    "low",
                    "open",
                    "marketposition",
                    "date",
                    "time",
                    "currentbar",
                ]
                .contains(&word.as_str())
                {
                    color = Some(0xa03080);
                }
            } else {
                i += 1;
            }
            position += chars[start..i]
                .iter()
                .map(|c| c.len_utf16() as i32)
                .sum::<i32>();
            if let Some(color) = color {
                paint(at, position, color);
            }
        }
        SendMessageW(self.code, 0x437, 0, &selection as *const _ as isize);
        SendMessageW(self.code, 0x445, 0, 1 | 4);
        SendMessageW(self.code, 0xb, 1, 0);
        InvalidateRect(self.code, null(), 0);
        self.update_gutter();
    }
    unsafe fn update_gutter(&self) {
        let first = SendMessageW(self.code, 0xce, 0, 0).max(0) as usize;
        let lines = text(self.code).lines().count().max(1);
        let labels = (first + 1..=(first + 60).min(lines))
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if text(self.line_numbers) != labels {
            set(self.line_numbers, &labels);
        }
    }
    unsafe fn syntax_check(&mut self) -> Result<(), String> {
        self.checkpoint()?;
        let doc = &self.documents[self.active];
        let mut functions = std::collections::BTreeMap::new();
        if let Some(files) = doc.task["function_files"].as_object() {
            for (name, path) in files {
                let path = core::absolute(&self.root, path.as_str().ok_or("函式來源路徑無效")?);
                functions.insert(
                    name.to_owned(),
                    fs::read_to_string(path).map_err(|e| e.to_string())?,
                );
            }
        }
        if let Some(inline) = doc.task["parameters"]["functions"].as_object() {
            if !functions.is_empty() && !inline.is_empty() {
                return Err("functions 與 function_files 不可同時提供".into());
            }
            for (name, source) in inline {
                functions.insert(
                    name.to_owned(),
                    source.as_str().ok_or("函式內容須為文字")?.to_owned(),
                );
            }
        }
        let inputs = serde_json::from_value(if doc.kind == "function" {
            json!({})
        } else {
            doc.task["parameters"]["inputs"].clone()
        })
        .map_err(|e| e.to_string())?;
        if doc.kind == "function" {
            functions.insert(doc.name.to_owned(), doc.source.clone());
        }
        match recharts_core::powerlanguage::Program::with_functions(
            if doc.kind == "function" {
                ""
            } else {
                &doc.source
            },
            &inputs,
            &functions,
        ) {
            Ok(_) => {
                self.append(&format!(
                    "{}：語法檢查完成（相容引擎；非原廠 MC 編譯）。",
                    doc.name
                ));
                set(self.status, "語法檢查通過，可開始回測");
                Ok(())
            }
            Err(e) => {
                self.append(&format!("{}：{e}", doc.name));
                set(self.status, "語法檢查未通過，請查看下方輸出");
                Err(e)
            }
        }
    }
    unsafe fn collect(&self) -> Result<Value, String> {
        let mut task: Value = serde_json::from_str(&text(self.advanced))
            .map_err(|e| format!("完整任務 JSON：{e}"))?;
        if !task["parameters"].is_object() {
            return Err("任務缺少 parameters".into());
        }
        for f in &self.fields {
            let value = text(f.edit).trim().to_owned();
            if ["symbol", "bar_unit", "bar_period"].contains(&f.key) {
                if !task["market"].is_object() {
                    task["market"] = json!({});
                }
                match f.key {
                    "symbol" => task["market"]["symbol"] = json!(value),
                    "bar_unit" => {
                        task["market"]["unit"] = json!(match value.as_str() {
                            "秒 K" => "seconds",
                            "分 K" => "minutes",
                            _ => "days",
                        })
                    }
                    _ => {
                        task["market"]["period"] =
                            json!(value.parse::<u64>().map_err(|_| "週期須為正整數")?)
                    }
                }
                continue;
            }
            task["parameters"][f.key] = match f.key {
                "strategy_name" | "from" | "to" => json!(value),
                "source_interval_minutes" if value.is_empty() => Value::Null,
                "initial_capital" | "commission_per_side" => {
                    let n = value
                        .parse::<f64>()
                        .map_err(|_| format!("{} 須為有效數值", f.key))?;
                    if !n.is_finite() {
                        return Err("數值不可為無限大".into());
                    }
                    json!(n)
                }
                _ => json!(value
                    .parse::<u64>()
                    .map_err(|_| format!("{} 須為非負整數", f.key))?),
            };
        }
        let inputs = text(self.inputs);
        task["parameters"]["inputs"] = serde_json::from_str(if inputs.trim().is_empty() {
            "{}"
        } else {
            inputs.trim()
        })
        .map_err(|e| format!("策略輸入值 JSON：{e}"))?;
        if !task["parameters"]["inputs"].is_object() {
            return Err("策略輸入值須為 JSON 物件".into());
        }
        Ok(task)
    }
    unsafe fn show_page(&mut self, page: usize) {
        self.page = page;
        ShowWindow(self.code_hint, if page == 0 { 5 } else { 0 });
        ShowWindow(self.code, if page == 0 { 5 } else { 0 });
        for h in self.fields.iter().flat_map(|f| [f.label, f.edit]).chain([
            self.inputs_label,
            self.inputs,
            self.advanced_label,
            self.advanced,
            self.datasets,
        ]) {
            ShowWindow(h, if page == 1 { 5 } else { 0 });
        }
        for h in [self.result_summary]
            .into_iter()
            .chain(self.open_buttons.iter().copied())
        {
            ShowWindow(h, if page == 2 { 5 } else { 0 });
        }
        ShowWindow(self.line_numbers, if page == 0 { 5 } else { 0 });
        ShowWindow(self.editor_tabs, 5);
        ShowWindow(self.logs, 5);
        for (i, h) in self.tabs.iter().enumerate() {
            EnableWindow(*h, if i == page { 0 } else { 1 });
        }
        for h in self
            .data_fields
            .iter()
            .flat_map(|f| [f.label, f.edit])
            .chain([self.data_hint, self.data_list])
            .chain(self.data_buttons.iter().copied())
        {
            ShowWindow(h, if page == 3 { 5 } else { 0 });
        }
        self.layout();
    }
    unsafe fn layout(&self) {
        let mut r: Rect = zeroed();
        GetClientRect(self.window, &mut r);
        let full_width = r.right;
        let w = full_width - 300;
        let h = r.bottom;
        let bottom = h - 275;
        for (i, b) in self.toolbar.iter().enumerate() {
            position(*b, 12 + i as i32 * 124, 8, 116, 32);
        }
        position(self.notice, 14, 47, full_width - 28, 28);
        for (i, b) in self.tabs.iter().enumerate() {
            position(*b, 14 + i as i32 * 160, 82, 152, 30);
        }
        position(self.editor_tabs, 14, 124, w - 28, 32);
        position(self.code_hint, 14, 165, w - 28, 27);
        position(self.line_numbers, 14, 197, 48, bottom - 197);
        position(self.code, 64, 197, w - 78, bottom - 197);
        position(self.navigator_label, w + 10, 124, 276, 26);
        position(self.search, w + 10, 157, 276, 28);
        position(self.library_kind, w + 10, 193, 276, 160);
        position(self.library_list, w + 10, 232, 276, bottom - 232);
        let col = (w - 42) / 3;
        for (i, f) in self.fields.iter().enumerate() {
            let x = 14 + (i % 3) as i32 * (col + 7);
            let y = 172 + (i / 3) as i32 * 58;
            position(f.label, x, y, col, 24);
            position(
                f.edit,
                x,
                y + 25,
                col,
                if f.key == "bar_unit" { 160 } else { 28 },
            );
        }
        position(self.inputs_label, 14, 410, w - 210, 24);
        position(self.datasets, w - 205, 408, 190, 30);
        position(self.inputs, 14, 442, w - 28, 60);
        position(self.advanced_label, 14, 510, w - 28, 24);
        position(self.advanced, 14, 538, w - 28, bottom - 538);
        for (i, b) in self.open_buttons.iter().enumerate() {
            position(*b, 14 + i as i32 * 175, 172, 168, 34);
        }
        position(self.result_summary, 14, 220, w - 28, bottom - 220);
        position(self.output_label, 14, bottom + 12, full_width - 28, 24);
        position(self.logs, 14, bottom + 40, full_width - 28, 145);
        position(self.status, 14, h - 29, full_width - 28, 24);
        position(self.excel, 14, h - 72, 225, 28);
        position(self.directory_label, 244, h - 68, 115, 26);
        position(self.directory, 362, h - 72, full_width - 376, 28);
        position(self.data_hint, 14, 172, w - 28, 40);
        let half = (w - 42) / 2;
        for (i, f) in self.data_fields.iter().enumerate() {
            let (x, y, width) = if i < 4 {
                (
                    14 + (i % 2) as i32 * (half + 14),
                    220 + (i / 2) as i32 * 68,
                    half,
                )
            } else {
                (14, 356, w - 28)
            };
            position(f.label, x, y, width, 24);
            position(
                f.edit,
                x,
                y + 25,
                width,
                if ["kind", "role"].contains(&f.key) {
                    160
                } else {
                    28
                },
            );
        }
        for (i, b) in self.data_buttons.iter().enumerate() {
            position(*b, 14 + i as i32 * 230, 418, 220, 32);
        }
        position(self.data_list, 14, 464, w - 28, bottom - 464);
    }
    unsafe fn append(&self, s: &str) {
        let n = GetWindowTextLengthW(self.logs);
        SendMessageW(self.logs, 0xb1, n as usize, n as isize);
        SendMessageW(
            self.logs,
            0xc2,
            0,
            wide(&(s.replace('\n', "\r\n") + "\r\n")).as_ptr() as isize,
        );
        SendMessageW(self.logs, 0xb7, 0, 0);
    }
    unsafe fn lock(&mut self, busy: bool) {
        self.busy = busy;
        for h in [
            self.library_list,
            self.search,
            self.library_kind,
            self.editor_tabs,
        ] {
            EnableWindow(h, if busy { 0 } else { 1 });
        }
        for h in self
            .data_fields
            .iter()
            .map(|f| f.edit)
            .chain(self.data_buttons.iter().copied())
        {
            EnableWindow(h, if busy { 0 } else { 1 });
        }
        for h in self.toolbar.iter().chain([&self.excel, &self.directory]) {
            EnableWindow(*h, if busy { 0 } else { 1 });
        }
        for h in self
            .fields
            .iter()
            .map(|f| f.edit)
            .chain([self.inputs, self.advanced, self.code])
        {
            SendMessageW(h, 0xcf, if busy { 1 } else { 0 }, 0);
        }
    }
    unsafe fn start(&mut self) -> Result<(), String> {
        if self.busy {
            return Ok(());
        }
        self.checkpoint()?;
        if self
            .documents
            .get(self.active)
            .is_some_and(|d| d.kind == "function")
        {
            return Err("函式不可單獨回測，請在策略的相依函式設定中使用".into());
        }
        let task = self.collect()?;
        let source = text(self.code);
        if source.trim().is_empty() {
            return Err("請輸入 PL 程式碼".into());
        }
        let excel = SendMessageW(self.excel, 0xf0, 0, 0) == 1;
        let dir = text(self.directory);
        let root = self.root.clone();
        self.result = None;
        set(self.result_summary, "回測執行中，完成後會顯示本次結果。");
        self.show_page(2);
        self.lock(true);
        self.append(&format!("開始回測：{}", self.documents[self.active].name));
        set(self.status, "回測執行中…");
        let (send, recv) = mpsc::channel();
        self.receiver = Some(recv);
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                core::run(
                    &root,
                    task,
                    &source,
                    excel,
                    if dir.trim().is_empty() {
                        None
                    } else {
                        Some(Path::new(&dir))
                    },
                    |s| {
                        let _ = send.send(Event::Log(s));
                    },
                )
            })
            .unwrap_or_else(|_| Err("回測工作執行緒發生錯誤".into()));
            let _ = send.send(Event::Done(result));
        });
        Ok(())
    }
    unsafe fn poll(&mut self) {
        if self
            .changed_at
            .is_some_and(|at| at.elapsed() > Duration::from_millis(350))
        {
            self.highlight();
            self.changed_at = None;
        }
        self.update_gutter();
        if !self.busy
            && self.session_pending
            && self.last_session_save.elapsed() > Duration::from_secs(2)
        {
            if let Err(e) = self.persist_session() {
                self.last_session_save = Instant::now();
                set(self.status, &format!("草稿尚未保存：{e}"));
            }
        }
        let events: Vec<_> = self
            .receiver
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Imported(result) => {
                    self.lock(false);
                    self.receiver = None;
                    match result {
                        Ok(r) => {
                            for doc in &mut self.documents {
                                doc.result = None;
                            }
                            self.result = None;
                            self.update_result_summary();
                            self.session_pending = true;
                            let _ = self.refresh_sources();
                            set(
                                self.status,
                                &format!(
                                    "匯入完成｜{}｜本次 {} 筆｜目前共 {} 筆",
                                    r["symbol"].as_str().unwrap_or(""),
                                    r["input_rows"],
                                    r["total_rows"]
                                ),
                            );
                            self.append(&format!("匯入收據：{r}"));
                        }
                        Err(e) => {
                            set(self.status, "匯入失敗；原有行情未被部分覆蓋");
                            alert(self.window, &e);
                        }
                    }
                }
                Event::Log(s) => self.append(&s),
                Event::Done(result) => {
                    self.lock(false);
                    self.receiver = None;
                    match result {
                        Ok(r) => {
                            if r["status"] == "completed" {
                                set(
                                    self.status,
                                    &format!(
                                        "完成｜已平倉 {} 筆｜淨利 {:.2}｜最大回撤 {:.2}",
                                        r["summary"]["trades"],
                                        r["summary"]["net_closed"].as_f64().unwrap_or(0.),
                                        r["summary"]["max_drawdown"].as_f64().unwrap_or(0.)
                                    ),
                                );
                            } else {
                                set(self.status, "執行失敗，已保留本次紀錄與已完成的報表");
                                self.append(r["error"].as_str().unwrap_or("未知錯誤"));
                            }
                            self.append(&format!(
                                "結果索引：{}",
                                r["result_path"].as_str().unwrap_or("")
                            ));
                            self.result = Some(r);
                            if let Some(doc) = self.documents.get_mut(self.active) {
                                doc.result = self.result.clone();
                            }
                            self.update_result_summary();
                            self.session_pending = true;
                        }
                        Err(e) => {
                            set(self.status, "設定或回測執行失敗");
                            set(self.result_summary, &format!("回測未完成\n{e}"));
                            self.append(&e);
                        }
                    }
                }
            }
        }
    }
    unsafe fn dialog(&self, save: bool, task: bool) -> Option<PathBuf> {
        let mut filename = vec![0u16; 32768];
        let filter = wide(if self.page == 3 && !task {
            "行情資料 (*.csv;*.txt)\0*.csv;*.txt\0所有檔案\0*.*\0"
        } else if task {
            "回測設定 (*.json)\0*.json\0所有檔案\0*.*\0"
        } else {
            "PL 程式碼 (*.pl)\0*.pl;*.txt\0所有檔案\0*.*\0"
        });
        let initial = wide(
            &self
                .root
                .join(if task { "tasks" } else { "strategies" })
                .to_string_lossy(),
        );
        let ext = wide(if task { "json" } else { "pl" });
        let mut f: OpenFile = zeroed();
        f.size = size_of::<OpenFile>() as u32;
        f.owner = self.window;
        f.filter = filter.as_ptr();
        f.file = filename.as_mut_ptr();
        f.max_file = filename.len() as u32;
        f.initial = initial.as_ptr();
        f.default_ext = ext.as_ptr();
        f.flags = 0x80000 | 0x800 | 8 | if save { 2 } else { 0x1000 };
        let success = if save {
            GetSaveFileNameW(&mut f)
        } else {
            GetOpenFileNameW(&mut f)
        };
        if success == 0 {
            None
        } else {
            Some(PathBuf::from(String::from_utf16_lossy(
                &filename[..filename
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(filename.len())],
            )))
        }
    }
    unsafe fn command(&mut self, id: usize) -> Result<(), String> {
        if self.busy && [101, 102, 103, 140, 142, 143, 144, 147, 148].contains(&id) {
            return Err("工作執行中，請等待完成後再操作策略。".into());
        }
        match id {
            101 if !self.busy => {
                if let Some(p) = self.dialog(false, true) {
                    self.checkpoint()?;
                    self.load(&p)?;
                    let task = self.collect()?;
                    let name = task["parameters"]["strategy_name"]
                        .as_str()
                        .unwrap_or("策略")
                        .to_owned();
                    let origin = task["strategy"].as_str().map(PathBuf::from);
                    self.documents.push(Document {
                        id: format!("task:{}", p.display()),
                        name,
                        kind: "strategy".into(),
                        source: text(self.code),
                        task,
                        origin,
                        managed_id: None,
                        dirty: false,
                        result: None,
                    });
                    self.active = self.documents.len() - 1;
                    self.result = None;
                    self.refresh_editor_tabs();
                    self.session_pending = true;
                    self.highlight();
                    self.show_page(0);
                    SetFocus(self.code);
                }
            }
            102 if !self.busy => {
                let page = self.page;
                self.page = 0;
                let file = self.dialog(false, false);
                self.page = page;
                if let Some(p) = file {
                    self.checkpoint()?;
                    let mut task = self.collect()?;
                    let name = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("匯入策略")
                        .trim_start_matches("s_")
                        .trim_start_matches("f_")
                        .to_owned();
                    task["parameters"]["strategy_name"] = json!(name);
                    task["parameters"]["inputs"] = json!({});
                    task["parameters"]["simple_variables"] = json!([]);
                    let kind = if SendMessageW(self.library_kind, 0x147, 0, 0) == 1 {
                        "function"
                    } else {
                        "strategy"
                    };
                    self.documents.push(Document {
                        id: format!("import:{}:{}", p.display(), self.documents.len()),
                        name,
                        kind: kind.into(),
                        source: fs::read_to_string(&p)
                            .map_err(|e| e.to_string())?
                            .trim_start_matches('\u{feff}')
                            .to_owned(),
                        task,
                        origin: Some(p.clone()),
                        managed_id: None,
                        dirty: true,
                        result: None,
                    });
                    self.active = self.documents.len() - 1;
                    self.display_document()?;
                    self.refresh_editor_tabs();
                    self.session_pending = true;
                    self.show_page(0);
                    SetFocus(self.code);
                    set(
                        self.status,
                        "PL 已匯入並顯示；確認程式碼與回測設定後，按「開始回測」。",
                    );
                }
            }
            103 if !self.busy => {
                self.save_document()?;
            }
            140 => {
                self.checkpoint()?;
                let mut doc = library::new_document(&self.root, self.documents.len() + 1)?;
                let mut task = self.collect()?;
                task["parameters"]["strategy_name"] = json!(doc.name);
                task["parameters"]["inputs"] = json!({});
                task["parameters"]["simple_variables"] = json!([]);
                doc.task = task;
                self.documents.push(doc);
                self.active = self.documents.len() - 1;
                self.display_document()?;
                self.refresh_editor_tabs();
                self.show_page(0);
                self.session_pending = true;
                SetFocus(self.code);
            }
            142 => {
                self.checkpoint()?;
                let doc = &self.documents[self.active];
                if doc.dirty {
                    let answer = MessageBoxW(
                        self.window,
                        wide(&format!("是否儲存「{}」？", doc.name)).as_ptr(),
                        wide("關閉分頁").as_ptr(),
                        0x23,
                    );
                    if answer == 2 {
                        return Ok(());
                    }
                    if answer == 6 {
                        self.save_document()?;
                    } else {
                        let dir = library::library_dir(&self.root).join("closed-drafts");
                        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                        core::write_json(
                            &dir.join(format!(
                                "{}.json",
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_nanos()
                            )),
                            &json!(self.documents[self.active]),
                        )?;
                    }
                }
                self.documents.remove(self.active);
                if self.documents.is_empty() {
                    self.documents.push(library::new_document(&self.root, 1)?);
                }
                self.active = self.active.min(self.documents.len() - 1);
                self.display_document()?;
                self.refresh_editor_tabs();
                self.session_pending = true;
            }
            143 => {
                let page = self.page;
                self.page = 0;
                let file = self.dialog(true, false);
                self.page = page;
                if let Some(path) = file {
                    let normalized = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    if self.entries.iter().any(|e| {
                        !e.managed
                            && fs::canonicalize(&e.source)
                                .unwrap_or_else(|_| e.source.clone())
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&normalized.to_string_lossy())
                    }) {
                        return Err("此檔案是原始來源，請另存到其他位置，原始策略不覆寫。".into());
                    }
                    fs::write(path, text(self.code)).map_err(|e| e.to_string())?;
                }
            }
            144 => self.syntax_check()?,
            145 => self.code_changed(),
            146 => self.refresh_library(),
            147 => {
                let index = SendMessageW(self.editor_tabs, 0x130b, 0, 0);
                if index >= 0 {
                    self.switch_document(index as usize)?;
                }
            }
            148 => self.open_selected()?,
            104 => self.start()?,
            105 => {
                self.refresh_sources()?;
                self.show_page(3);
            }
            110..=113 => {
                self.show_page(id - 110);
                if id == 113 {
                    self.refresh_sources()?;
                }
            }
            130 if !self.busy => {
                if let Some(p) = self.dialog(false, false) {
                    set(self.data_fields[4].edit, &p.to_string_lossy());
                }
            }
            131 => self.import_data()?,
            132 => self.refresh_sources()?,
            133 => self.sync_import_metadata()?,
            134 => {
                let godot_script=self.root.join("quote-manager-godot.ps1");
                if godot_script.is_file() {
                    let args=format!("-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\" -Database \"{}\"",godot_script.display(),self.data_database()?.display());
                    if ShellExecuteW(self.window,wide("open").as_ptr(),wide("powershell.exe").as_ptr(),wide(&args).as_ptr(),wide(&self.root.to_string_lossy()).as_ptr(),0)<=32{return Err("無法開啟Godot商品資料管理".into());}
                    return Ok(());
                }
                let mut exe=self.root.join("rust/target/release/quote_manager_gui.exe");
                if !exe.is_file(){exe=self.root.join("rust/target/debug/quote_manager_gui.exe");}
                if !exe.is_file(){return Err("請先編譯quote_manager.exe或使用行情資料管理入口".into());}
                let args=format!("--database=\"{}\"",self.data_database()?.display());
                if ShellExecuteW(self.window,wide("open").as_ptr(),wide(&exe.to_string_lossy()).as_ptr(),wide(&args).as_ptr(),wide(&self.root.to_string_lossy()).as_ptr(),5)<=32{return Err("無法開啟商品資料管理".into());}
            },
            120..=123 => {
                let r = self.result.as_ref().ok_or("請先完成一次回測")?;
                let path = if id == 123 {
                    Path::new(r["result_path"].as_str().ok_or("缺少本次目錄")?)
                        .parent()
                        .unwrap()
                        .to_path_buf()
                } else {
                    PathBuf::from(
                        r["artifacts"][["html", "excel", "trades"][id - 120]]
                            .as_str()
                            .ok_or("本次未產出此檔案")?,
                    )
                };
                if ShellExecuteW(
                    self.window,
                    wide("open").as_ptr(),
                    wide(&path.to_string_lossy()).as_ptr(),
                    null(),
                    null(),
                    1,
                ) <= 32
                {
                    return Err("無法開啟檔案，請從本次資料夾查看".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
}
unsafe extern "system" fn procedure(window: H, message: u32, w: usize, l: isize) -> isize {
    let id = w & 0xffff;
    let event = w >> 16;
    let handled_change = (event == 1 && (id == 301 || id == 402))
        || (event == 0x300 && (id == 201 || id == 401))
        || (event == 2 && id == 403);
    if message == 0x111 && event != 0 && !handled_change {
        return DefWindowProcW(window, message, w, l);
    }
    match message {
        5 => {
            UI.with(|cell| {
                if let Ok(b) = cell.try_borrow() {
                    if let Some(ui) = b.as_ref() {
                        ui.layout();
                    }
                }
            });
            0
        }
        0x111 => {
            UI.with(|cell| {
                if let Ok(mut b) = cell.try_borrow_mut() {
                    if let Some(ui) = b.as_mut() {
                        let command = match (id, event) {
                            (301, 1) => 133,
                            (402, 1) | (401, 0x300) => 146,
                            (201, 0x300) => 145,
                            (403, 2) => 148,
                            _ => id,
                        };
                        if let Err(e) = ui.command(command) {
                            alert(window, &e);
                        }
                    }
                }
            });
            0
        }
        0x4e => {
            let notification = l as *const Notify;
            if !notification.is_null()
                && (*notification).id == 404
                && (*notification).code == (-551i32) as u32
            {
                UI.with(|c| {
                    if let Ok(mut b) = c.try_borrow_mut() {
                        if let Some(ui) = b.as_mut() {
                            if let Err(e) = ui.command(147) {
                                alert(window, &e);
                            }
                        }
                    }
                });
            }
            0
        }
        0x113 => {
            UI.with(|cell| {
                if let Ok(mut b) = cell.try_borrow_mut() {
                    if let Some(ui) = b.as_mut() {
                        ui.poll();
                    }
                }
            });
            0
        }
        0x10 => {
            let busy = UI.with(|c| {
                c.try_borrow()
                    .map(|v| v.as_ref().is_some_and(|u| u.busy))
                    .unwrap_or(true)
            });
            if busy {
                alert(window, "回測執行中，請等待本次回測完成後再關閉。");
            } else {
                let saved = UI.with(|c| {
                    if let Ok(mut b) = c.try_borrow_mut() {
                        if let Some(ui) = b.as_mut() {
                            if let Err(e) = ui.persist_session() {
                                alert(window, &format!("草稿尚未保存，未關閉視窗：{e}"));
                                return false;
                            }
                        }
                    }
                    true
                });
                if saved {
                    DestroyWindow(window);
                }
            }
            0
        }
        2 => {
            PostQuitMessage(0);
            0
        }
        0x24 => {
            let points = l as *mut Point;
            if !points.is_null() {
                (*points.add(3)).x = 1180;
                (*points.add(3)).y = 940;
            }
            0
        }
        _ => DefWindowProcW(window, message, w, l),
    }
}
pub unsafe fn run() {
    SetProcessDpiAwarenessContext(-4);
    let common = CommonControls {
        size: size_of::<CommonControls>() as u32,
        classes: 8,
    };
    if InitCommonControlsEx(&common) == 0 || LoadLibraryW(wide("Msftedit.dll").as_ptr()) == 0 {
        alert(0, "Windows 原生編輯元件無法載入。");
        return;
    }
    let name = wide("REA_PL_Rust_Workbench");
    let instance = GetModuleHandleW(null());
    let class = Class {
        style: 3,
        proc: procedure,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: 0,
        cursor: LoadCursorW(0, 32512usize as *const u16),
        background: 16,
        menu: null(),
        name: name.as_ptr(),
    };
    if RegisterClassW(&class) == 0 {
        alert(0, "無法註冊工作台視窗");
        return;
    }
    let menu = CreateMenu();
    for (title, items) in [
        (
            "檔案",
            vec![
                (140, "新增策略"),
                (102, "匯入 PL"),
                (103, "儲存策略"),
                (143, "匯出 PL"),
                (142, "關閉分頁"),
            ],
        ),
        ("策略", vec![(144, "檢查語法"), (104, "開始回測")]),
        (
            "檢視",
            vec![
                (110, "程式編輯"),
                (111, "回測設定"),
                (113, "行情資料／匯入"),
                (112, "回測結果"),
            ],
        ),
        (
            "報表",
            vec![(120, "HTML 報表"), (121, "Excel 報表"), (122, "交易明細")],
        ),
    ] {
        let popup = CreatePopupMenu();
        for (id, label) in items {
            AppendMenuW(popup, 0, id, wide(label).as_ptr());
        }
        AppendMenuW(menu, 0x10, popup as usize, wide(title).as_ptr());
    }
    let window = CreateWindowExW(
        0,
        name.as_ptr(),
        wide("PL 策略管理｜Rust 工作台").as_ptr(),
        0x00cf0000,
        80,
        50,
        1540,
        980,
        0,
        0,
        instance,
        null(),
    );
    if window == 0 {
        alert(0, "無法建立工作台視窗");
        return;
    }
    SetMenu(window, menu);
    match Ui::new(window) {
        Ok(ui) => UI.with(|c| *c.borrow_mut() = Some(ui)),
        Err(e) => {
            alert(window, &e);
            DestroyWindow(window);
            return;
        }
    }
    ShowWindow(window, 5);
    UpdateWindow(window);
    SetTimer(window, 1, 150, 0);
    let mut msg: Msg = zeroed();
    while GetMessageW(&mut msg, 0, 0, 0) > 0 {
        if msg.message == 0x100 && GetKeyState(0x11) < 0 {
            let command = match msg.wparam {
                0x53 => Some(103),
                0x4e => Some(140),
                0x4f => Some(102),
                _ => None,
            };
            if let Some(id) = command {
                SendMessageW(window, 0x111, id, 0);
                continue;
            }
            if msg.wparam == 0x56 {
                let is_code =
                    UI.with(|c| c.borrow().as_ref().is_some_and(|ui| GetFocus() == ui.code));
                if is_code {
                    SendMessageW(GetFocus(), 0x440, 13, 0);
                    continue;
                }
            }
        }
        if msg.message == 0x100 && msg.wparam == 0x41 && GetKeyState(0x11) < 0 {
            let focus = GetFocus();
            let editable = UI.with(|c| {
                c.borrow().as_ref().is_some_and(|ui| {
                    [ui.code, ui.inputs, ui.advanced, ui.directory, ui.search].contains(&focus)
                        || ui.fields.iter().any(|field| field.edit == focus)
                })
            });
            if editable {
                SendMessageW(focus, 0xb1, 0, -1);
                continue;
            }
        }
        if IsDialogMessageW(window, &msg) == 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    UI.with(|c| {
        if let Some(ui) = c.borrow_mut().take() {
            DeleteObject(ui.font);
            DeleteObject(ui.mono);
        }
    });
}
