# Godot 行情資料中心

2026-11-01：行情資料管理介面已更新為 Godot 4，資料作業仍由既有 Rust 模組執行。原圖表研究工作站 `godot/` 保留，新介面位於 `godot-quote/`，不會變更原圖表主場景。

## 開啟與操作

- 雙擊根目錄 `行情資料管理.cmd`，預設開啟 Godot 新介面。工作台「商品／資料庫管理」按鈕亦已切換。
- 左側可搜尋／選擇商品；中央三張卡片顯示 Tick、1分K、日K實際來源筆數、起訖及時間標記。滑鼠移到時間標記可查看時區及衍生資料筆數。
- 「完整性與資料作業」：輸入日期，執行缺漏檢查，顯示各來源涵蓋率與缺漏區段；可按 Tick 補建缺少K棒。
- 「匯入與匯出」：選來源、時區、時間標記及檔案，匯入前會列出覆蓋規則；匯出拒絕覆寫既有檔案。
- 「商品規格與交易時段」：新增商品、價格規格、任意多個交易時段、開盤星期、交易日位移及手動休市日。勾選自動補建後須儲存才生效。
- 切換商品／重新整理／關閉前會提醒尚未儲存的修改。背景作業執行期間鎖定編輯，完成後顯示結果；執行期間請等待完成再關閉。
- 初始選擇TXF1（若存在），日期預設為該商品最新Tick日期，沒有Tick時取其他來源最新日期；不是整段歷史已完成檢查。

## 架構與建置

`godot-quote/main.gd` 負責介面；`coverage.gd` 繪製涵蓋率。`rust/src/quote_manager_bridge.rs` 透過 GDExtension 提供 `QuoteManagerBridge.start/poll`，標準 Rust 背景執行緒呼叫既有 `quote_manager`，只透過通道傳回 JSON，不從背景執行緒存取 Godot 物件。

```powershell
./quote-manager-godot.ps1 -Build
./quote-manager-godot.ps1 -Database C:/path/market.sqlite
./quote-manager.ps1 -Legacy
./test-quote-manager-godot.ps1
```

`-Build` 需先關閉本次新版視窗，避免Windows鎖住已載入的DLL。建置採離線Rust依賴。舊Win32介面保留於 `-Legacy`。啟動器使用專案 `.tools/godot` 既有Godot與 `.tools/appdata`，不另外安裝全域軟體。

## 驗證與範圍

112項Rust核心測試通過。Godot端到端腳本以獨立 `outputs/godot-ui-test-*` 資料庫，實際經過介面欄位與Rust橋接：新增商品、儲存、匯入4筆Tick（含合法重複與收盤整點成交）、自動補建2筆1分K及1筆日K、三來源清單、缺少1分鐘、再次補建與匯出4筆Tick。每次保留 `verification.json` 與 `godot.log`；需驗證明確成功標記，不能僅憑程序結束認定通過。

原有資料規則不變，詳見 [商品與行情資料管理](quote-manager.md)。Tick無成交僅為疑似缺漏；無成交不造棒；日K完整性取決於來源Tick；沒有自動交易所休市曆、QMD或即時行情服務。新介面不代表所有策略與原生MC完整相容。

已實際檢查三分頁與正式TXF1唯讀查詢：2026-10-23完整性掃描38.4秒完成，涵蓋率圖及缺漏區段正常顯示；期間視窗保持可回應。資料來源Tick132,942,684筆／1分K609,032筆／日K657筆及既有時間標記均保持。交付紀錄 `outputs/godot-quote-delivery-20261101.json`；最終端到端證據 `outputs/godot-ui-test-66e6506549a844e9af5053656748589d/`。
