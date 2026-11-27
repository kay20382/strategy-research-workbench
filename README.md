# 策略研究工作台

適用於 Windows 的期貨策略研究與交易管理工具。從匯入行情、編寫 PowerLanguage（PL）到回測、比對版本與 AI 優化，都可以在同一個工作台完成。Codex 和 Claude Code 也能透過 MCP 使用同一套研究工具。桌面介面使用 Godot，行情與回測核心使用 Rust，資料存於 SQLite。

## 可以做什麼

| 工作區 | 用途 |
| --- | --- |
| 對話首頁 | 選擇 AI 模型，用提示詞查詢策略、發起回測或請助理協助撰寫 PL。 |
| 行情資料庫 | 管理商品、匯入及匯出 Tick／1 分 K／日 K，檢查行情涵蓋範圍與缺漏。 |
| 策略工作區 | 編輯 PL、檢查語法、設定回測、查看成交與權益曲線，以及進行 AI 優化和版本比對。PL 可匯出為 MultiCharts 常用的 Windows Big5／CP950 編碼。 |
| 即時 K 線 | 選擇連續月商品、日期區間及 Tick／秒／分／日週期，顯示既有行情並接續即時更新。可套用策略顯示回測進出場。 |
| 策略執行與交易回報 | 以「策略＋商品」建立正式執行項目，分別設定策略參數及商品週期，查看券商委託與成交。 |
| 設定 | 集中管理 AI、行情與交易 API、Telegram。AI 來源包含 OpenAI、Claude、Gemini、Grok 和 MixRoute。 |

## 從 GitHub 開始

以下指令在 Windows PowerShell 執行。先使用具備儲存庫存取權的 GitHub 帳號，並安裝 Git、Rust MSVC 工具鏈及 Visual Studio C++ 建置工具（含 Windows SDK）。桌面介面另需 Godot 4.7.2 Windows x64。

```powershell
git clone https://github.com/kay20382/strategy-research-workbench.git
Set-Location strategy-research-workbench
cargo build --locked --manifest-path rust/Cargo.toml --bin strategy_mcp --bin backtest --bin pl_workbench_cli --bin trading
```

只使用 MCP 可跳到下一節。要開啟桌面工作台，再從儲存庫根目錄執行以下指令。將第一行改為你下載的 Godot 主控台執行檔的完整路徑。

```powershell
$godotConsole = 'C:\Tools\Godot\Godot_v4.7.2-stable_win64_console.exe'
cargo build --locked --manifest-path rust/Cargo.toml --release --lib
New-Item -ItemType Directory -Force godot-workbench/bin | Out-Null
Copy-Item rust/target/release/recharts_core.dll godot-workbench/bin/recharts_core-ide-diff.dll -Force
& $godotConsole --headless --path godot-workbench --editor --import --quit
& $godotConsole --path godot-workbench
```

若既有開發環境已配置 `.tools/` 內的 Godot、Rust 和離線 Cargo 套件，可直接雙擊 `PL回測工作台.cmd`。在新複製的儲存庫首次回測前，先到「行情資料庫」建立或選擇商品並匯入行情。Excel 和部分報表匯出另外需要 Node.js、Python、`@oai/artifact-tool` 和本機報表設定。

## 從 Codex 或 Claude Code 使用 MCP

MCP 由用戶端啟動 `strategy_mcp.exe`，透過本機標準輸入與輸出連線。桌面工作台可以關閉。以下兩種設定的路徑都要換成你的實際路徑。

**Codex**，在 `config.toml` 加入設定，再重新啟動用戶端或開啟新的工作階段。

```toml
[mcp_servers.strategy_studio]
command = "C:/Projects/strategy-research-workbench/rust/target/debug/strategy_mcp.exe"
tool_timeout_sec = 1800
```

**Claude Code**，從儲存庫根目錄註冊並檢查。

```powershell
claude mcp add --transport stdio strategy_studio -- "C:/Projects/strategy-research-workbench/rust/target/debug/strategy_mcp.exe"
claude mcp list
```

連線後可先請助理「透過 `strategy_studio` 列出策略」，驗證 `strategy_list` 有回應。正式研究時先確認商品、週期、交易規則和績效門檻。條件模糊時，`strategy_requirements` 會回傳題目、建議選項及「自行描述」，待你確認驗收清單後開始。助理可讀基準報表、提交候選 PL、逐輪評分，再交付最佳版本、每輪紀錄及程式碼對照。 [MCP 工具與研究流程](docs/strategy-studio-mcp.md)

## 第一次回測

1. 在「行情資料庫」匯入商品行情，核對來源、起訖時間及缺漏。
2. 在「策略工作區」開啟或匯入 PL，設定商品、K 棒週期、期間、成本及暖機棒數。按 **F7** 檢查語法。
3. 按 **F5** 執行回測，核對逐筆成交、淨損益、最大回撤及期末部位。要比較候選版本，沿用同一組行情、成本與設定。
4. 檢查成交棒是否符合 `exit_bar > entry_bar`。如有同棒進出，研究流程會保留明細供核對，該結果無法成為合格基準或最佳版本。

下單需另外設定券商元件、帳戶及連線，再到「策略執行」啟用。行情資料庫、正式帳密、API 金鑰、憑證及回測輸出由各開發者在本機配置。回測引擎支援已實作的 PL 語法與函式，與 MultiCharts 的完整成交及報表等價仍待全面驗證。 [券商 API 設定](docs/api-profiles.md) · [行情來源與彙整](docs/market-source-integration.md)

## 專案結構

| 路徑 | 內容 |
| --- | --- |
| [rust/](rust/) | 行情、回測、策略研究、MCP 與 Godot 橋接程式 |
| [godot-workbench/](godot-workbench/) | 桌面工作台介面 |
| [native/](native/) | 券商行情與交易轉接程式 |
| [strategies/](strategies/) · [tasks/](tasks/) | 策略原始碼與回測任務範例 |
| [scripts/](scripts/) · [docs/](docs/) | 報表、驗證工具與操作文件 |
