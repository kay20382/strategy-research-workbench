//! Rust原生Win32商品資料管理；耗時工作使用背景執行緒。
use recharts_core::{
    quote_manager::{self as qm, Instrument, TradingSession},
    workbench,
};
use serde_json::Value;
use std::{
    cell::RefCell,
    ffi::c_void,
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::null,
    sync::mpsc::{self, Receiver},
};
type H = isize;
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
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
    name_menu: *const u16,
    name: *const u16,
}
#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
struct OpenFile {
    size: u32,
    owner: H,
    instance: H,
    filter: *const u16,
    custom: *mut u16,
    max_custom: u32,
    index: u32,
    file: *mut u16,
    max_file: u32,
    title_file: *mut u16,
    max_title: u32,
    initial: *const u16,
    title: *const u16,
    flags: u32,
    offset: u16,
    extension: u16,
    default_ext: *const u16,
    data: isize,
    hook: usize,
    template: *const u16,
    reserved: *mut c_void,
    reserved2: u32,
    flags_ex: u32,
}
#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(c: *const Class) -> u16;
    fn CreateWindowExW(
        ex: u32,
        c: *const u16,
        t: *const u16,
        s: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        p: H,
        m: H,
        i: H,
        v: *const c_void,
    ) -> H;
    fn DefWindowProcW(h: H, m: u32, w: usize, l: isize) -> isize;
    fn ShowWindow(h: H, n: i32) -> i32;
    fn UpdateWindow(h: H) -> i32;
    fn GetMessageW(m: *mut Msg, h: H, a: u32, b: u32) -> i32;
    fn TranslateMessage(m: *const Msg) -> i32;
    fn DispatchMessageW(m: *const Msg) -> isize;
    fn PostQuitMessage(n: i32);
    fn DestroyWindow(h: H) -> i32;
    fn SetWindowTextW(h: H, s: *const u16) -> i32;
    fn GetWindowTextLengthW(h: H) -> i32;
    fn GetWindowTextW(h: H, s: *mut u16, n: i32) -> i32;
    fn SendMessageW(h: H, m: u32, w: usize, l: isize) -> isize;
    fn MoveWindow(h: H, x: i32, y: i32, w: i32, n: i32, r: i32) -> i32;
    fn GetClientRect(h: H, r: *mut Rect) -> i32;
    fn LoadCursorW(i: H, n: *const u16) -> H;
    fn MessageBoxW(h: H, t: *const u16, c: *const u16, f: u32) -> i32;
    fn SetTimer(h: H, id: usize, n: u32, f: usize) -> usize;
    fn EnableWindow(h: H, b: i32) -> i32;
    fn IsDialogMessageW(h: H, m: *const Msg) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(s: *const u16) -> H;
}
#[link(name = "gdi32")]
extern "system" {
    fn CreateFontW(
        a: i32,
        b: i32,
        c: i32,
        d: i32,
        e: i32,
        f: u32,
        g: u32,
        h: u32,
        i: u32,
        j: u32,
        k: u32,
        l: u32,
        m: u32,
        n: *const u16,
    ) -> H;
    fn DeleteObject(h: H) -> i32;
}
#[link(name = "comdlg32")]
extern "system" {
    fn GetOpenFileNameW(f: *mut OpenFile) -> i32;
    fn GetSaveFileNameW(f: *mut OpenFile) -> i32;
}
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn text(h: H) -> String {
    let mut b = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
    String::from_utf16_lossy(&b[..n as usize])
}
unsafe fn set(h: H, s: &str) {
    SetWindowTextW(h, w(&s.replace('\n', "\r\n")).as_ptr());
}
struct Field {
    edit: H,
}
struct Ui {
    window: H,
    font: H,
    db: PathBuf,
    list: H,
    status: H,
    output: H,
    fields: Vec<Field>,
    buttons: Vec<(H, i32, i32, i32)>,
    items: Vec<Instrument>,
    receiver: Option<Receiver<Result<Value, String>>>,
    kind: H,
    auto: H,
    dirty: bool,
}
thread_local! {static UI:RefCell<Option<Ui>>=const{RefCell::new(None)};}
unsafe fn child(
    parent: H,
    class: &str,
    title: &str,
    style: u32,
    id: usize,
    font: H,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> H {
    let h = CreateWindowExW(
        if class == "EDIT" { 0x200 } else { 0 },
        w(class).as_ptr(),
        w(title).as_ptr(),
        0x50000000 | style,
        x,
        y,
        width,
        height,
        parent,
        id as H,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(h, 0x30, font as usize, 1);
    if class == "EDIT" {
        SendMessageW(h, 0xc5, 2_000_000, 0);
    }
    h
}
impl Ui {
    unsafe fn new(window: H) -> Result<Self, String> {
        let font = CreateFontW(
            -17,
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
            w("Microsoft JhengHei UI").as_ptr(),
        );
        let root = workbench::root();
        let db = std::env::args()
            .skip(1)
            .find_map(|a| a.strip_prefix("--database=").map(PathBuf::from))
            .unwrap_or(root.join("data/market.sqlite"));
        qm::initialize(&root, &db)?;
        child(
            window,
            "STATIC",
            "商品清單｜Tick／1分K／日K",
            0,
            0,
            font,
            12,
            12,
            340,
            25,
        );
        let list = child(
            window,
            "LISTBOX",
            "",
            0x10000 | 0x200000 | 1,
            100,
            font,
            12,
            42,
            340,
            650,
        );
        let mut u = Self {
            window,
            font,
            db,
            list,
            status: 0,
            output: 0,
            fields: vec![],
            buttons: vec![],
            items: vec![],
            receiver: None,
            kind: 0,
            auto: 0,
            dirty: false,
        };
        for (label, x, y, width, height) in [
            ("商品代碼", 370, 12, 155, 26),
            ("商品名稱", 540, 12, 215, 26),
            ("商品時區（保留原時鐘）", 770, 12, 225, 26),
            ("PriceScale", 370, 70, 155, 26),
            ("MinMove", 540, 70, 155, 26),
            ("每點價值", 710, 70, 155, 26),
            ("日盤名稱（空白停用）", 370, 132, 155, 26),
            ("開盤 HH:MM", 540, 132, 110, 26),
            ("收盤 HH:MM", 665, 132, 110, 26),
            ("開盤星期1~7", 790, 132, 110, 26),
            ("交易日位移0/1", 915, 132, 115, 26),
            ("夜盤名稱（空白停用）", 370, 194, 155, 26),
            ("開盤 HH:MM", 540, 194, 110, 26),
            ("收盤 HH:MM", 665, 194, 110, 26),
            ("開盤星期1~7", 790, 194, 110, 26),
            ("交易日位移0/1", 915, 194, 115, 26),
            ("休市開盤日期（逗號分隔YYYY-MM-DD）", 370, 256, 450, 26),
            ("交易日星期1~7", 835, 256, 195, 26),
            ("檢查／補建／匯出 起日", 370, 370, 200, 26),
            ("迄日（補建／匯出使用）", 585, 370, 200, 26),
            ("檔案路徑（匯入或新建匯出）", 370, 432, 660, 26),
            ("來源時區（沿用既有）", 370, 494, 280, 26),
            ("時間標記 event／end／start／unspecified", 665, 494, 365, 26),
        ] {
            let n = u.fields.len();
            child(window, "STATIC", label, 0, 0, font, x, y, width + 25, 22);
            let eh = child(
                window,
                "EDIT",
                "",
                0x10000 | 0x80,
                200 + n,
                font,
                x,
                y + 23,
                width,
                height,
            );
            u.fields.push(Field { edit: eh });
        }
        for (title, id, x, y, width) in [
            ("新增商品", 101, 12, 710, 105),
            ("重新整理", 102, 128, 710, 105),
            ("儲存商品／時段", 103, 370, 318, 190),
            ("讀取商品", 104, 572, 318, 140),
            ("選擇檔案", 105, 1044, 455, 110),
            ("匯入資料", 106, 370, 558, 125),
            ("匯出CSV", 107, 508, 558, 125),
            ("檢查缺漏", 108, 646, 558, 125),
            ("Tick補缺K棒", 109, 784, 558, 150),
            ("儲存來源標記", 112, 947, 558, 170),
        ] {
            let h = child(window, "BUTTON", title, 0x10000, id, font, x, y, width, 31);
            u.buttons.push((h, x, y, width));
        }
        u.auto = child(
            window,
            "BUTTON",
            "匯入Tick後自動補建缺少的1分K／日K",
            0x10000 | 3,
            110,
            font,
            370,
            350,
            650,
            22,
        );
        child(window, "STATIC", "資料格式", 0, 0, font, 1045, 370, 120, 22);
        u.kind = child(
            window,
            "COMBOBOX",
            "",
            0x10000 | 3,
            111,
            font,
            1045,
            393,
            125,
            180,
        );
        for value in ["Tick", "1 分 K", "日 K"] {
            SendMessageW(u.kind, 0x143, 0, w(value).as_ptr() as isize);
        }
        SendMessageW(u.kind, 0x14e, 1, 0);
        u.output=child(window,"EDIT","選擇商品後，設定日期範圍並檢查缺漏。Tick無成交分鐘僅列疑似缺漏。\n交易日位移1會移至下一交易日；休市日須自行維護。",0x10000|0x300000|4|0x800|0x40,120,font,370,610,800,200);
        u.status = child(
            window,
            "STATIC",
            &format!("資料庫：{}", u.db.display()),
            0,
            0,
            font,
            12,
            830,
            1150,
            25,
        );
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let end = chrono::DateTime::from_timestamp(seconds + 8 * 3600, 0)
            .unwrap()
            .date_naive();
        set(
            u.fields[18].edit,
            &(end - chrono::Duration::days(30)).to_string(),
        );
        set(u.fields[19].edit, &end.to_string());
        u.apply(&qm::defaults("TXF1"));
        u.refresh()?;
        u.sync_meta()?;
        Ok(u)
    }
    unsafe fn apply(&mut self, i: &Instrument) {
        for (n, v) in [
            (0, i.symbol.clone()),
            (1, i.name.clone()),
            (2, i.timezone.clone()),
            (3, i.price_scale.to_string()),
            (4, i.minimum_move.to_string()),
            (5, i.point_value.to_string()),
            (16, i.closed_dates.join(",")),
            (
                17,
                i.trading_weekdays
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(""),
            ),
        ] {
            set(self.fields[n].edit, &v);
        }
        for row in 0..2 {
            let at = 6 + row * 5;
            if let Some(s) = i.sessions.get(row) {
                for (n, v) in [
                    s.name.clone(),
                    s.open.clone(),
                    s.close.clone(),
                    s.weekdays
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(""),
                    s.trading_day_shift.to_string(),
                ]
                .into_iter()
                .enumerate()
                {
                    set(self.fields[at + n].edit, &v);
                }
            } else {
                for n in at..at + 5 {
                    set(self.fields[n].edit, "");
                }
            }
        }
        SendMessageW(self.auto, 0xf1, i.auto_fill as usize, 0);
        self.dirty = false;
    }
    unsafe fn selected_kind(&self) -> &'static str {
        match SendMessageW(self.kind, 0x147, 0, 0) {
            0 => "tick",
            1 => "minute",
            _ => "daily",
        }
    }
    unsafe fn refresh(&mut self) -> Result<(), String> {
        let v = qm::inventory(&self.db)?;
        self.items.clear();
        SendMessageW(self.list, 0x184, 0, 0);
        for row in v.as_array().unwrap() {
            let i: Instrument =
                serde_json::from_value(row["instrument"].clone()).map_err(|e| e.to_string())?;
            let sources = row["sources"].as_array().unwrap();
            let flags = [("tick", "T"), ("minute", "M"), ("daily", "D")]
                .iter()
                .map(|(k, label)| {
                    if sources.iter().any(|r| r["kind"] == *k) {
                        *label
                    } else {
                        "—"
                    }
                })
                .collect::<Vec<_>>()
                .join(" /");
            SendMessageW(
                self.list,
                0x180,
                0,
                w(&format!(
                    "{}   {}   {}{}",
                    i.symbol,
                    flags,
                    i.name,
                    if i.confirmed { "" } else { "〔待確認〕" }
                ))
                .as_ptr() as isize,
            );
            self.items.push(i);
        }
        Ok(())
    }
    unsafe fn load_symbol(&mut self) -> Result<(), String> {
        let n = SendMessageW(self.list, 0x188, 0, 0);
        if n < 0 {
            return Ok(());
        }
        let i = self.items[n as usize].clone();
        self.apply(&i);
        self.sync_meta()?;
        let inventory = qm::inventory(&self.db)?;
        let row = inventory
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["instrument"]["symbol"] == i.symbol)
            .unwrap();
        let mut msg = format!("{}｜{}\n", i.symbol, i.name);
        if !i.confirmed {
            msg+="此主檔仍為預設範本，請確認價格規格與交易時段後儲存；未確認前不套用回測，也不自動補建。\n\n";
        }
        for (kind, label) in [("tick", "Tick"), ("minute", "1分K"), ("daily", "日K")] {
            if let Some(s) = row["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["kind"] == kind)
            {
                msg+=&format!("{}：{}筆\n{} ～ {}\n時區 {}；時間標記 {}\nTick衍生 {}筆；來源Tick已異動 {}筆（不自動覆寫，需重新核對）\n\n",label,s["row_count"],s["first_datetime"].as_str().unwrap_or(""),s["last_datetime"].as_str().unwrap_or(""),s["timezone"],s["timestamp_role"],s["tick_derived_rows"],s["tick_source_changed_rows"]);
            } else {
                msg += &format!("{label}：尚無資料\n");
            }
        }
        set(self.output, &msg);
        Ok(())
    }
    unsafe fn sync_meta(&self) -> Result<(), String> {
        let symbol = text(self.fields[0].edit).trim().to_uppercase();
        let v = qm::inventory(&self.db)?;
        let kind = self.selected_kind();
        if let Some(s) = v
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["instrument"]["symbol"] == symbol)
            .and_then(|r| r["sources"].as_array())
            .and_then(|a| a.iter().find(|r| r["kind"] == kind))
        {
            set(
                self.fields[21].edit,
                s["timezone"].as_str().unwrap_or("unspecified"),
            );
            set(
                self.fields[22].edit,
                s["timestamp_role"].as_str().unwrap_or("unspecified"),
            );
        } else {
            set(self.fields[21].edit, &text(self.fields[2].edit));
            set(
                self.fields[22].edit,
                if kind == "tick" { "event" } else { "end" },
            );
        }
        Ok(())
    }
    unsafe fn instrument(&self) -> Result<Instrument, String> {
        let get = |n: usize| text(self.fields[n].edit).trim().to_owned();
        let weekdays = |s: &str| -> Result<Vec<u32>, String> {
            let mut v = vec![];
            for c in s.chars().filter(|c| !c.is_whitespace() && *c != ',') {
                let n = c
                    .to_digit(10)
                    .filter(|n| (1..=7).contains(n))
                    .ok_or("星期請填12345，或1234567")?;
                if !v.contains(&n) {
                    v.push(n);
                }
            }
            Ok(v)
        };
        let mut sessions = vec![];
        for row in 0..2 {
            let n = 6 + row * 5;
            if !get(n).is_empty() {
                sessions.push(TradingSession {
                    name: get(n),
                    open: get(n + 1),
                    close: get(n + 2),
                    weekdays: weekdays(&get(n + 3))?,
                    trading_day_shift: get(n + 4).parse().map_err(|_| "交易日位移須為0或1")?,
                });
            }
        }
        if let Ok(existing) = qm::get(&self.db, &get(0)) {
            if existing.sessions.len() > 2 {
                return Err("此商品超過兩個時段，請用CLI商品JSON編輯，避免UI截斷設定".into());
            }
        }
        Ok(Instrument {
            confirmed: true,
            symbol: get(0),
            name: get(1),
            english_name: qm::get(&self.db,&get(0)).map(|i|i.english_name).unwrap_or_default(),
            timezone: get(2),
            price_scale: get(3).parse().map_err(|_| "PriceScale錯誤")?,
            minimum_move: get(4).parse().map_err(|_| "MinMove錯誤")?,
            point_value: get(5).parse().map_err(|_| "每點價值錯誤")?,
            sessions,
            closed_dates: get(16)
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            trading_weekdays: weekdays(&get(17))?,
            auto_fill: SendMessageW(self.auto, 0xf0, 0, 0) == 1,
        })
    }
    unsafe fn choose(&self, save: bool) -> Option<PathBuf> {
        let mut buf = vec![0u16; 32768];
        let mut f: OpenFile = zeroed();
        let filter = w("行情CSV／TXT\0*.csv;*.txt\0所有檔案\0*.*\0");
        let ext = w("csv");
        f.size = size_of::<OpenFile>() as u32;
        f.owner = self.window;
        f.filter = filter.as_ptr();
        f.file = buf.as_mut_ptr();
        f.max_file = buf.len() as u32;
        f.default_ext = ext.as_ptr();
        f.flags = 0x80000 | 8 | if save { 2 } else { 0x1000 };
        if (if save {
            GetSaveFileNameW(&mut f)
        } else {
            GetOpenFileNameW(&mut f)
        }) != 0
        {
            Some(PathBuf::from(String::from_utf16_lossy(
                &buf[..buf.iter().position(|v| *v == 0).unwrap_or(0)],
            )))
        } else {
            None
        }
    }
    unsafe fn command(&mut self, id: usize) -> Result<(), String> {
        if self.receiver.is_some() {
            return Ok(());
        }
        match id {
            101 => {
                self.apply(&qm::defaults("NEW_SYMBOL"));
                set(
                    self.output,
                    "請設定商品代碼、價格規格與交易時段，再按儲存。",
                );
                self.dirty = true;
            }
            102 => self.refresh()?,
            103 => {
                qm::save(&self.db, &self.instrument()?)?;
                self.refresh()?;
                self.sync_meta()?;
                self.dirty = false;
                set(self.status, "商品與時段已儲存，回測載入此商品時套用。");
            }
            104 => self.load_symbol()?,
            112 => {
                let v = qm::annotate_source(
                    &self.db,
                    &text(self.fields[0].edit),
                    self.selected_kind(),
                    &text(self.fields[21].edit),
                    &text(self.fields[22].edit),
                )?;
                set(self.output, &format_result(&v));
                set(
                    self.status,
                    "來源時區／時間標記已記錄；原始時間未轉換。補建前請確認各來源一致。",
                );
            }
            105 => {
                if let Some(p) = self.choose(false) {
                    set(self.fields[20].edit, &p.to_string_lossy());
                }
            }
            106..=109 => {
                let db = self.db.clone();
                let symbol = text(self.fields[0].edit).trim().to_uppercase();
                let _ = qm::get(&db, &symbol)?;
                let from = text(self.fields[18].edit).trim().to_owned();
                let to = text(self.fields[19].edit).trim().to_owned();
                let kind = self.selected_kind().to_owned();
                let zone = text(self.fields[21].edit).trim().to_owned();
                let role = text(self.fields[22].edit).trim().to_owned();
                let path = if id == 107 {
                    match self.choose(true) {
                        Some(p) => p,
                        None => return Ok(()),
                    }
                } else {
                    PathBuf::from(text(self.fields[20].edit).trim())
                };
                if id == 107 {
                    set(self.fields[20].edit, &path.to_string_lossy());
                }
                let (send, recv) = mpsc::channel();
                self.receiver = Some(recv);
                for (h, _, _, _) in &self.buttons {
                    EnableWindow(*h, 0);
                }
                EnableWindow(self.list, 0);
                set(self.status, "處理中…資料量大時請稍候，可繼續查看視窗。");
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(|| match id {
                        106 => qm::import(&db, &path, &symbol, &kind, &zone, &role),
                        107 => qm::export(&db, &symbol, &kind, &from, &to, &path),
                        108 => qm::coverage_all(&db, &symbol),
                        _ => qm::fill_from_ticks(&db, &symbol, &from, &to),
                    })
                    .unwrap_or_else(|_| Err("背景工作發生錯誤，請查看資料庫與來源".into()));
                    let _ = send.send(result);
                });
            }
            _ => {}
        }
        Ok(())
    }
    unsafe fn poll(&mut self) {
        let result = self.receiver.as_ref().and_then(|r| r.try_recv().ok());
        if let Some(r) = result {
            self.receiver = None;
            for (h, _, _, _) in &self.buttons {
                EnableWindow(*h, 1);
            }
            EnableWindow(self.list, 1);
            match r {
                Ok(v) => {
                    set(self.output, &format_result(&v));
                    set(self.status, "完成。既有報表保留；行情異動後請重新回測。");
                    let _ = self.refresh();
                }
                Err(e) => {
                    set(self.output, &e);
                    set(self.status, "未完成，請依上方錯誤修正。");
                }
            }
        }
    }
    unsafe fn layout(&self) {
        let mut r: Rect = zeroed();
        GetClientRect(self.window, &mut r);
        let height = r.bottom.max(780);
        MoveWindow(self.list, 12, 42, 340, (height - 132).max(100), 1);
        for (h, x, _, width) in &self.buttons {
            if *x < 350 {
                MoveWindow(*h, *x, height - 78, *width, 31, 1);
            }
        }
        MoveWindow(
            self.output,
            370,
            610,
            (r.right - 385).max(660),
            (height - 655).max(100),
            1,
        );
        MoveWindow(self.status, 12, height - 34, r.right - 25, 28, 1);
    }
}
fn format_result(v: &Value) -> String {
    fn value_text(x: &Value) -> &str {
        x.as_str().unwrap_or("")
    }
    if let Some(sources) = v["sources"].as_array() {
        let mut s = format!(
            "{}｜{} ～ {}\n{}\n\n",
            value_text(&v["symbol"]),
            value_text(&v["from"]),
            value_text(&v["to"]),
            value_text(&v["calendar"])
        );
        for r in sources {
            s += &format!(
                "{}：預期 {}，有資料 {}，缺少／疑似 {}；共 {} 區段\n{}\n{}\n",
                match value_text(&r["kind"]) {
                    "tick" => "Tick",
                    "minute" => "1分K",
                    _ => "日K",
                },
                r["expected"],
                r["observed"],
                r["missing"],
                r["gap_count"],
                value_text(&r["meaning"]),
                value_text(&r["warning"])
            );
            for gap in r["gaps"].as_array().unwrap() {
                s += &format!(
                    "  {} ～ {}  共{}\n",
                    value_text(&gap["from"]),
                    value_text(&gap["to"]),
                    gap["count"]
                );
            }
            s.push('\n');
        }
        s
    } else if !v["inserted"].is_null() {
        format!("{}｜Tick補建完成\n{} ～ {}\n讀取Tick：{}筆\n新增1分K：{}根\n新增日K：{}根\n\n只補缺少的時間，既有K棒保留。\n{}",value_text(&v["symbol"]),value_text(&v["from"]),value_text(&v["to"]),v["tick_rows_read"],v["inserted"]["minute"],v["inserted"]["daily"],value_text(&v["warning"]))
    } else if !v["import"].is_null() {
        let a = &v["import"];
        let mut result = format!(
            "{}｜匯入完成\n輸入 {} 筆，來源總共 {} 筆\n{} ～ {}\n",
            value_text(&a["symbol"]),
            a["input_rows"],
            a["total_rows"],
            value_text(&a["first"]),
            value_text(&a["last"])
        );
        if v["auto_fill"]["status"] == "failed" {
            result += &format!(
                "\n原始匯入已保存，但自動補建未完成：{}\n請統一來源時區與時間標記後，再執行補建。",
                value_text(&v["auto_fill"]["error"])
            );
        } else if v["auto_fill"]["status"] == "completed" {
            let a = v["auto_fill"]["result"].as_array().unwrap();
            let m: i64 = a
                .iter()
                .map(|x| x["inserted"]["minute"].as_i64().unwrap_or(0))
                .sum();
            let d: i64 = a
                .iter()
                .map(|x| x["inserted"]["daily"].as_i64().unwrap_or(0))
                .sum();
            result += &format!("\n自動新增1分K {m}根、日K {d}根；請檢查Tick本身是否缺漏。");
        }
        result
    } else if !v["rows"].is_null() {
        format!(
            "匯出完成：{}筆\n{}\n{} ～ {}",
            v["rows"],
            value_text(&v["path"]),
            value_text(&v["from"]),
            value_text(&v["to"])
        )
    } else if v["timestamps_unchanged"] == true {
        format!(
            "來源標記已儲存\n時區：{}\n時間標記：{}\n原始時間值未轉換；本次操作已有紀錄。",
            value_text(&v["timezone"]),
            value_text(&v["role"])
        )
    } else {
        v.to_string()
    }
}
unsafe extern "system" fn proc(h: H, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        0x111 => {
            let id = wp & 0xffff;
            let notification = wp >> 16;
            if id >= 200 || id == 120 {
                return 0;
            }
            let error = UI.with(|cell| {
                let Ok(mut slot) = cell.try_borrow_mut() else {
                    return None;
                };
                if let Some(u) = slot.as_mut() {
                    let result = if id == 100 && notification == 1 {
                        u.load_symbol()
                    } else if id == 111 && notification == 1 {
                        u.sync_meta()
                    } else {
                        u.command(id)
                    };
                    result.err()
                } else {
                    None
                }
            });
            if let Some(e) = error {
                MessageBoxW(h, w(&e).as_ptr(), w("行情資料管理").as_ptr(), 0x30);
            }
            0
        }
        0x5 => {
            UI.with(|c| {
                let Ok(slot) = c.try_borrow() else {
                    return;
                };
                if let Some(u) = slot.as_ref() {
                    u.layout();
                }
            });
            0
        }
        0x113 => {
            UI.with(|c| {
                let Ok(mut slot) = c.try_borrow_mut() else {
                    return;
                };
                if let Some(u) = slot.as_mut() {
                    u.poll();
                }
            });
            0
        }
        0x10 => {
            let busy = UI.with(|c| c.borrow().as_ref().is_some_and(|u| u.receiver.is_some()));
            if busy {
                MessageBoxW(
                    h,
                    w("正在提交資料，請待工作完成後再關閉。").as_ptr(),
                    w("行情資料管理").as_ptr(),
                    0x30,
                );
            } else {
                DestroyWindow(h);
            }
            0
        }
        0x2 => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
pub unsafe fn run() {
    let instance = GetModuleHandleW(null());
    let name = w("REAQuoteManager");
    let class = Class {
        style: 3,
        proc,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: 0,
        cursor: LoadCursorW(0, 32512usize as *const u16),
        background: 6,
        name_menu: null(),
        name: name.as_ptr(),
    };
    RegisterClassW(&class);
    let window = CreateWindowExW(
        0,
        name.as_ptr(),
        w("行情資料管理｜Rust Quote Manager").as_ptr(),
        0x00cf0000,
        80,
        60,
        1220,
        960,
        0,
        0,
        instance,
        null(),
    );
    match Ui::new(window) {
        Ok(u) => UI.with(|c| *c.borrow_mut() = Some(u)),
        Err(e) => {
            MessageBoxW(window, w(&e).as_ptr(), w("無法啟動").as_ptr(), 0x10);
            DestroyWindow(window);
            return;
        }
    }
    ShowWindow(window, 5);
    UpdateWindow(window);
    SetTimer(window, 1, 200, 0);
    UI.with(|c| {
        if let Some(u) = c.borrow().as_ref() {
            u.layout();
        }
    });
    let mut msg: Msg = zeroed();
    while GetMessageW(&mut msg, 0, 0, 0) > 0 {
        if IsDialogMessageW(window, &msg) == 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    UI.with(|c| {
        if let Some(u) = c.borrow_mut().take() {
            DeleteObject(u.font);
        }
    });
}
