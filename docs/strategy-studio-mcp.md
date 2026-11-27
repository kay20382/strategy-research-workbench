# 策略研究工作台 MCP

工作台內建 AI 遇到需要釐清的策略作業時，會分輪提問。每題可點選建議答案、其他選項，或選「自行描述」輸入自己的條件，再按「送出本輪答案」。驗收清單整理完成後，回覆「確認驗收」才會進入撰寫、優化或回測。需要中止時可回覆「取消驗收」。明確要求沿用既有設定的「開始回測」可直接執行。

透過 MCP 處理模糊的策略需求時，用戶端先呼叫 `strategy_requirements`。可提供 `strategy_id` 讀取既有設定，並在 `request` 傳入原始需求。MCP 回傳 `questions`、各題的完整選項、`recommended` 建議標記與 `free_text` 自行描述標記。Codex 與工作台照原文呈現。依題目的 `key` 回填 `acceptance`，可查下一輪仍未確認的項目。問題全部解決後，再請使用者確認驗收清單。直接以完整結構化參數呼叫底層回測工具仍可用於自動化。

製作新策略時，一律指定 `mode: "new"`。問卷分開確認交易商品（`product`）、K 棒週期（`bar_period`）、交易時段及行情來源（`market_session`）、策略目標（`strategy_goal`）、預期進場（`entry_rules`）、預期出場（`exit_rules`）、停損（`stop_loss`）及停利（`take_profit`）。已清楚指定的條件可直接回填，再確認整份驗收清單。

Codex 或 Claude Code 負責讀報表、依研究準則評分、改寫 PL 並驅動迴圈。MCP 共用桌面工作台的 Rust 回測引擎，負責執行、比對和保存紀錄。用戶端透過本機標準輸入與輸出（stdio）連線。 [MCP stdio 規格](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

## 中文原始碼與修改前後對照

產出的 PL 以繁體中文註釋說明交易規則。MCP 的 `strategy_export_pl` 接收策略庫 `strategy_id`、回測 `run_id` 或完整 `source`，三擇一，輸出 Windows Big5／CP950、CRLF 的 `.pl`。工作台可從「工具」選「匯出 PowerLanguage（CP950）」，匯入也支援 UTF-8 和 CP950。無法無損編碼時會列出錯誤。回測保留原本的 `input.pl`，並另提供 `source_cp950`。

`strategy_diff` 的 `before` 和 `after` 各自接受同樣三種來源。開啟回傳的 `report_path`，可用左右對照查看對齊的程式碼與行號，紅底表示刪除、綠底表示新增，較深底色標出替換行內的變更區段。「上一處／下一處」可跳到變更行。工作台「AI 優化」也可從已選候選版開啟相同報表。MCP 同時回傳結構化的新增、刪除筆數與各自行號。UTF-8 BOM 與 LF／CRLF 的差異不列為程式碼變更，檔尾換行差異另列。超大型區塊會原樣列出替換前後的行，並回傳 `coarse: true`。

連續 10 行以上的未變更區段預設收合，變更附近保留前後文。可點區段按鈕個別展開，或用上方按鈕一次展開。右側總覽、上一處／下一處與 F7／Shift+F7 都以整段變更為單位跳轉。

## 連線

### Codex

將以下設定加入 Codex 的 `config.toml`，再於此專案開啟新的 Codex 工作階段。1800 秒供較長的回測使用。 [Codex MCP 設定](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)

```toml
[mcp_servers.strategy_studio]
command = "C:/WorkSpace/Codex/REAMulticharts/rust/target/debug/strategy_mcp.exe"
tool_timeout_sec = 1800
```

若要由命令列直接驅動，可在專案內執行以下命令，並把提示中的策略、期間及評分目標替換成研究需求。

```powershell
codex exec --skip-git-repo-check -c 'mcp_servers.strategy_studio.command="C:/WorkSpace/Codex/REAMulticharts/rust/target/debug/strategy_mcp.exe"' -c 'mcp_servers.strategy_studio.tool_timeout_sec=1800' '用 策略研究 MCP 查找指定策略，先讀基準報表，再依我的準則執行優化迴圈。'
```

### Claude Code

從專案根目錄啟動 Claude Code。專案的 `.mcp.json` 已宣告伺服器。依 Claude Code 的專案連線提示啟用後，可用 `/mcp` 查看狀態。 [Claude Code MCP 設定](https://code.claude.com/docs/en/mcp)

## 研究迴圈

可直接交代「透過 策略研究 MCP，以指定策略和期間執行 3 輪優化。依最大回撤、已平倉淨利與交易筆數評分，交付最佳版本及每輪修改理由。」有既有編號時，交代「從 `loop_id` 接續」。

1. 用 `strategy_list`、`strategy_read` 找到策略。呼叫 `backtest_run`，用 `report_read` 閱讀基準。需要查核交易時，使用 `backtest_result`。
2. 助理依使用者目標建立固定評分準則，寫明尺度與限制，再給基準 0 至 100 分。使用 `loop_start` 登錄目標、準則、輪數上限、基準分數和評語。
3. 依 `next_action` 操作。收到 `submit` 時，用 `loop_submit` 提交 `next_round`、完整 PL 與修改假設。引擎核對相同行情、設定和相依函式，並保存候選結果或拒絕原因。
4. 收到 `score` 時，依該輪 `run_id` 用 `report_read` 讀取績效與證據，必要時查看交易明細。依同一準則評分後，用 `loop_score` 登錄分數和理由。每輪評語應指出交易筆數、期末未平倉或引擎警告如何影響判斷。
5. 達到目標、輪數上限或研究停止條件時，用 `loop_finish` 記錄原因。回報基準、最佳 `run_id`、變動的績效指標、評分依據及候選原始碼或報表路徑。

迴圈可跨工作階段接續。呼叫 `loop_status` 讀取基準、歷次修改、分數、目前最佳版本與下一步。需要取回最近一次遭拒的程式碼時，可傳入 `include_failed_source: true`。同輪重送相同程式碼與假設會返回既有紀錄。各迴圈資料儲存在 `outputs/strategy-research-loops`，回測原始碼與報表位於 `outputs/pl-workbench`。評分取決於呼叫端的準則，是否通過樣本外驗證取決於實際指定的回測區間。

## 工具

| 工具 | 用途 |
| --- | --- |
| `api_profile_list` | 查詢工作台共用 API 設定及憑證狀態。 |
| `api_profile_schema` | 查詢指定券商及用途的欄位定義、自訂欄位支援及轉接器狀態。 |
| `api_profile_save` | 新增或更新券商 API 設定，可更新或清除憑證。 |
| `market_sources` | 查詢行情來源、筆數、起訖時間和時區，可指定商品。 |
| `market_query` | 依商品、Tick／1 分 K／日 K 和時間查詢行情，每次至多 1,000 筆。 |
| `strategy_list` | 取得策略庫及函式的 id。 |
| `strategy_read` | 讀取指定策略或函式的 PL 原始碼與任務設定。 |
| `strategy_version` | 提交與讀取策略和參數的版本，可對指定 GitHub 分支擷取與推送。 |
| `strategy_check` | 檢查 PL 語法，可沿用策略的相依函式。 |
| `strategy_logic` | 檢查策略邏輯、輸出每棒流程節點與分支，可另存離線 HTML 報告及可編輯的 Excalidraw 流程圖。 |
| `backtest_run` | 依策略任務回測原始碼或傳入的 PL，可指定起訖時間、原始資金與暖機棒數。 |
| `backtest_result` | 依回測編號查看績效摘要及逐筆交易，每頁 100 筆。 |
| `optimization_compare` | 對外部 AI 提供的候選 PL 執行回測，傳回原版與候選版的實際績效及差值。 |
| `robustness_run` | 對既有 OHLC 回測做 K 棒擾動、區塊拔靴及成交壓力測試，保存各次情境結果。 |
| `report_read` | 依回測編號讀取報表與完整損益，可指定 HTML／CSV，並可附上 PL。 |
| `model_update_report` | 依兩個已完成的回測編號產出模型更新比較，可指定 HTML／CSV。 |
| `loop_start` | 建立有固定評分準則和輪數上限的研究迴圈。 |
| `loop_status` | 讀取進度、歷次結果、目前最佳與下一步。 |
| `loop_submit` | 回測候選 PL，保存修改假設、結果或錯誤。 |
| `loop_score` | 記錄助理的分數與評語。 |
| `loop_finish` | 保存停止原因與最佳版本。 |
| `job_start` | 提交背景工作並立即取得工作編號。 |
| `job_status` | 查詢背景工作階段、耗時與結果。 |
| `job_cancel` | 要求取消背景工作。 |

### 背景工作

回測、強健性檢測、候選比較、報表與研究迴圈提交可在背景執行，同一條 MCP 連線可繼續處理其他請求。原工具仍回傳相同的結果格式。需要立即取得工作編號時，呼叫 `job_start`，例如 `{"tool":"backtest_run","arguments":{"strategy_id":"_kyleTest1"}}`。持有 `job_id` 後，以 `job_status` 查詢目前階段、耗時及最終結果。呼叫 `job_cancel` 可要求停止，接著用 `job_status` 確認狀態為 `cancelled`。每條連線最多同時執行四項背景工作。背景工作編號限同一條連線使用，回測報表依 `run_id` 保存。

直接呼叫原工具時，用戶端可附上 `_meta.progressToken` 接收階段進度，並以原請求編號傳送 `notifications/cancelled`。進度值為遞增的階段更新次數。回測與報表匯出的子程序會接收停止要求，強健性檢測在情境之間檢查取消。 [MCP 進度](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/progress) · [MCP 取消](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)

### Tick 分頁

Tick 查詢依成交時間戳與 `ordinal` 排序。每筆 Tick 含 `ordinal`。當 `truncated` 為 `true`，保留相同商品、`kind`、`from` 和 `to`，並將回應的 `next_cursor` 原樣放入下一次查詢的 `cursor`。下一頁從該成交之後開始，同一時間戳的成交會按原始順序保留。最後一頁的 `next_cursor` 為 `null`。若查詢期間 Tick 資料經匯入或更新，舊游標會失效，請從第一頁重新查詢。

### 策略研究與連線設定

AI 優化範例流程為 `strategy_list` → `strategy_read` → `backtest_run` → `optimization_compare`。外部助理負責生成修改後的完整 PL 原始碼。比較工具接收 `baseline_run_id` 及 `source`，會驗證行情雜湊、引擎、實際回測設定及相依函式。差值以候選版減去基準版計算。原版與候選版分別存入 `outputs/pl-workbench`，包含本次原始碼、設定、JSON 和 HTML 報表。

行情查詢預設使用 `data/market.sqlite`。可在用戶端的 `args` 設定 `["--database", "C:/完整路徑/market.sqlite"]` 指定查詢用資料庫。回測及候選版比較使用各策略任務所載的資料庫。工具提供行情查詢、PL 驗證及回測輸出，行情異動與策略庫儲存由桌面工作台處理。

券商、用途與憑證欄位及 QuoteCom 啟動方式見 [API 連線設定](api-profiles.md)。

需要重建時，可在 Windows PowerShell 執行以下命令。

```powershell
$env:CARGO_HOME = Join-Path $PWD '.tools/cargo'
$env:RUSTUP_HOME = Join-Path $PWD '.tools/rustup'
& .tools/cargo/bin/cargo.exe build --manifest-path rust/Cargo.toml --offline --bin strategy_mcp --bin backtest
```

## 績效報表與模型更新比較

### 策略強健性檢測

桌面工作台在完成回測後，可從結果區的「策略強健性檢測」選擇次數、最小跳動數、區塊長度與種子。MCP 呼叫範例為 `{"run_id":"rust-…","seed":42,"repetitions":20,"noise_ticks":1,"block_bars":20}`，工具名稱為 `robustness_run`。

- OHLC 雜訊將每個價位各自加減指定的最小跳動數，並修正最高與最低價包住開收盤。時間與成交量沿用基準。
- 移動區塊拔靴以放回抽樣取得連續 K 棒，抽取區塊內的價格變動、棒形與量，接回基準時間軸。日內區塊避開時間缺口，區塊接縫以先前收盤為起點，目的時間軸的休盤或缺口保留原開盤落差。價格採最小跳動的加法變動，較長期間可能累積漂移。
- 成交壓力使用原行情，測試多一跳且佣金增加 25%、多兩跳且佣金增加 50%，以及切換棒內價序假設。

報告列出各組淨損益和最大回撤的第 5、50、95 百分位、正淨利與低於基準的次數，以及零交易、未平倉、同棒違規和執行失敗。最大回撤沿用引擎負值，越負代表越大的回撤。各情境的設定、結果及行情雜湊保存在 `outputs/strategy-robustness`。同一基準與種子可重跑。檢測使用快照，不改寫行情庫。逐筆撮合基準須另建 OHLC 基準，且須另外以未用於開發的真實行情做樣本外驗證。

### 策略與參數版本

從桌面工作台上方點「版本控制」，在左側策略庫雙擊策略，或切換已開啟的策略分頁。選取歷史版本後，右側可預覽 PL 和回測設定。填寫說明後按「提交版本」，會保存目前草稿的 PL、策略與回測參數、商品與週期，以及相依函式原文。本機歷史儲存在 `data/strategy-library/git-versions`。按「還原為草稿」會在新分頁開啟所選版本，沿用目前環境的資料庫與報表路徑。使用原始資料集編號的任務需在另一臺電腦核對映射。策略庫的 Ctrl+S 儲存和版本提交可各別操作。

GitHub 使用已安裝的 Git、既有儲存庫的 HTTPS 網址與 Git 憑證管理員。預設使用該策略的 `strategy-…` 分支。推送前確認儲存庫存取權限，提交內容會包含策略原始碼與相依函式。行情資料庫、券商設定和回測報表不在快照中。換電腦時可開新策略草稿，填入同一 GitHub 網址及原分支名稱，擷取後選「套用到目前分頁」即可沿用擷取的版本歷史。提交後，推送時繼續指定同一遠端分支。同步只允許快轉，歷史分岔時需將不同歷史分別保留。

MCP 工具 `strategy_version` 與桌面共用策略庫中同一策略的版本歷史。使用 `strategy_list` 取得 `strategy_id`，再傳入 `operation`。`list` 查版本索引，`commit` 接收 `message`，可選擇提供完整 `source`、要覆寫的 `parameters`，或替換用的 `inputs`。`read` 加入完整 `version` 取得快照，`draft` 則回傳從該版本建立的草稿供檢視或後續編輯。使用 `fetch` 或 `push` 時須提供 `remote`，可指定 `branch`，推送另須傳入 `confirmed: true`。MCP 提交後即可在桌面版本控制頁重新整理查看。未儲存到策略庫的桌面草稿以該草稿的版本歷史管理。

### 策略邏輯檢查與條件流程圖

工作台的「邏輯檢查與流程圖」解析目前分頁的 PowerLanguage 草稿，開啟含檢查結果的離線 HTML。圖中依每根 K 棒的評估順序顯示指派、條件的是／否分支、迴圈及委託種類、數量和時點。點選檢查結果的節點編號可跳至圖中對應步驟。

按 HTML 中的「下載可編輯的 Excalidraw 流程圖」取得與報告同目錄的 `flow.excalidraw`。菱形是條件或迴圈判斷；綠色矩形是委託，虛線表示靜態分析判定不可達。圖檔在本機產生，策略原始碼不會因匯出而上傳。

MCP 可呼叫 `strategy_logic`，傳入 `strategy_id` 或完整 `source`。同時提供時以策略庫設定載入相依函式，分析傳入的原始碼。預設回傳結構化檢查與有方向的流程邊，傳入 `export_html: true` 會另存至 `outputs/strategy-logic`，並回傳 `report_path` 和 `excalidraw_path`。可從 HTML 下載圖檔，再匯入 Excalidraw 編輯。固定或矛盾條件、空迴圈、固定非法口數、可能的多空委託衝突、重複標籤、缺少進出場及本棒收盤委託會標記在對應節點。動態條件與函式內部狀態需以回測查核，跨棒規則以實際成交驗收。

`report_read` 的 `performance` 含扣費前損益、全體成交佣金、已平倉成本、已平倉淨損益、日均、勝率、賺賠比、單日及累積最大回撤、逐年逐月已平倉損益及最長連虧月份。逐期損益按出場歸屬，零損益月中斷連虧。日均的分母為暖機後有行情的統計日，含有行情但無成交的日子。單日回撤上下界一致時提供確定值，資料不足時保留可核對的範圍。累積回撤為引擎摘要之回撤絕對值。

`report_read` 與 `model_update_report` 接受 `date_basis`。預設 `auto`，依回測當時保存的已確認商品時段、交易星期與手動休市日歸屬。日均、單日回撤與年月損益使用同一日期基準。舊回測、時區未確認或時段無法歸屬時，報表說明原因並依來源曆日統計。日 K 依資料日期標記。指定 `source_calendar` 可用來源曆日重算。HTML 與 CSV 的「設定」區段會列出日期基準與同口徑的月、年損益，原有相容報表區段沿用既有欄位與公式。

新的回測收據與 `report_read` 提供 `market_coverage`，桌面回測摘要也會顯示。它記錄本次使用的行情來源、資料集、實際 K 棒起訖（含暖機）、最後檢查時間、檢查範圍與當時是否過期。已知缺漏總量是上次檢查全期間的數字，重疊缺口另列節錄。無紀錄、來源版本不同或範圍不相符時，狀態會明示。此資訊在回測時唯讀擷取，沿用行情管理的檢查結果，不會例行重掃行情。

桌面工作台的「開啟 HTML 報表」會開啟本次回測的 `report.html`，在交易明細上方顯示逐筆驗收、交易日績效及行情完整性。月損益、年損益與重疊缺口可展開查看。原始引擎 HTML 保存在同一次執行的 `engine/report.html`，可供對照。

指定 `format` 才建立檔案，例如 `{"run_id":"rust-…","format":"html"}`。回傳 `export.path` 及支援目錄。完整版使用 REA 資料夾中 MC 原生完整報告的五個區段順序、欄列及圖表位置，欄位統計與 Excel 共用。HTML 在各區段原有位置顯示圖表。CSV 以區段和原列號保存表格資料，日期與時間保留 Excel 序號。MCP 補充損益與驗證紀錄收在「設定」，完整逐年與逐月已平倉損益可取得於回應的 `performance`。圖表重新以該次回測資料繪製，缺少可核對資料的欄位保留未提供狀態。匯出沿用 Node、artifact-tool 與 Python。

完整 HTML 的交易明細每頁最多 100 筆，進出場列成對顯示。第一頁隨主檔開啟，之後按頁碼讀取同目錄的 `report-trades/`，可離線開啟。搬移 HTML 時保留整個 `export.support_directory`，包含分頁與圖表。CSV 保留完整交易明細與原列號。

`model_update_report` 的例子是 `{"baseline_run_id":"rust-…","candidate_run_id":"rust-…","format":"csv"}`。輸出兩版識別、引擎與策略雜湊、策略輸入、條件差異、績效差值及按期間對齊的年度和月度比較。差值為更新後減更新前。行情或交易條件不同時明示，缺少期間或指標保留空值。省略格式時，兩個工具均只回傳結構化資料。

### 簡式績效報告

以 `report_read` 傳入 `{"run_id":"rust-…","format":"html","style":"simple"}`。將 `html` 改為 `csv` 可匯出表格與完整曲線數列。省略 `style` 時使用完整報告。

簡式版取用同一套 MC 欄位與統計，涵蓋策略績效總結果、平倉權益曲線、平倉權益曲線及績效拉回、總交易分析、月週期分析、年週期分析與設定。HTML 圖以日期為 X 軸，權益圖以初始權益為相對零點。CSV 保留每筆出場時間和含初始資金的平倉權益。「平倉權益曲線及績效拉回」合併區段的 C 欄為平倉權益，D 欄為當前權益減去歷史前高。HTML 的金額與比例顯示千分位及兩位小數，負值以紅色括號呈現。

桌面工作台與 MCP 逐筆驗收每筆已平倉交易，要求 `exit_bar > entry_bar`。回測完成與成交驗收分別記錄。驗收未通過時，原始成交、績效與 `trade-validation.json` 可依回測編號查閱，明細包含進出場棒及委託名稱。此結果無法建立研究基準、進入候選評分或成為最佳版。舊回測在用於研究前會重新查核原始成交。
