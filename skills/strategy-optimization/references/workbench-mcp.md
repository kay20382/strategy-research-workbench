# 工作台相容性與 MCP

## PL 與資料

專案依 `rust/src/powerlanguage.rs` 的 `FUNCTIONS`、`BUILTINS`、執行分支及測試判斷。以下是 2026/11/13 查閱結果，遇版本變動重新核對。MultiCharts 有同名函式時，仍須分別驗證 PL 編譯及本機逐棒語義。 [MultiCharts PowerLanguage 語法](https://www.multicharts.com/trading-software/index.php?title=Language_Elements)

| 用途 | 本機可用候選 | 實作注意 |
| --- | --- | --- |
| 均線 | `Average(Price,N)`、`AverageFC(Price,N)`、`XAverage(Price,N)` | `XAverage` 在策略開始計算時以價格起算。EMA 交叉可由兩條 EMA 組合，另驗證訊號線及各自種子 |
| 突破 | `Highest`、`Lowest`、`HighestFC`、`LowestFC`、`HighestBar`、`LowestBar` | 先確定是否含當棒及同價取哪根。次棒掛單和當棒收盤確認是不同策略 |
| 波動 | `TrueRange`、`AvgTrueRange(N)`、`StdDev(Price,N)` | 本機 `AvgTrueRange` 按現行相容分支採 TR 窗口平均。Wilder 平滑須另定義。現行 `StdDev` 使用母體口徑 |
| 動能與統計 | `RSI(Price,N)`、`LinearRegSlope(Price,N)`、`Summation(Series,N)`、`CountIf(Condition,N)` | `RSI` 使用固定整數長度，全平初始化目前回報錯誤。迴歸斜率用人工上升與下降序列核對方向 |
| 量價及轉折 | `OBV`、`Volume`、`SwingHigh`、`SwingLow` | `OBV` 是內建名稱。轉折訊號要在右側棒確認後使用。日夜盤及成交量欄位來源先核對 |

ADX、VWAP、Volume Profile 等要查本機是否有可重用函式或是否需要新增計算。缺乏真實主動方向或委託簿時，保留資料缺口。資料管理可以查看多個商品，策略引擎仍須另外確認 Data2／外部序列的載入、同步與可用時間。未支援時可做資料規格及候選設計，待功能驗證後再執行該策略。

## MCP 呼叫順序

1. `strategy_list` → `strategy_read` → `backtest_run` → `report_read`。`backtest_run` 可指定 `from`、`to`、`initial_capital`、`warmup_bars`。回測前後都核對這些條件，尤其同名範例策略。必要時用 `backtest_result` 逐頁核對交易。
2. `loop_start` 傳 `baseline_run_id`、`goal`、`rubric`、`max_rounds`、`baseline_score`、`baseline_review`。比較單一候選也可使用 `optimization_compare`。
3. 依 `loop_status` 的 `next_action` 操作。提交前呼叫 `strategy_check`。 `loop_submit` 傳 `loop_id`、`round`、完整 `source`、`hypothesis`。成功才取得候選 `run_id`，失敗讀實際錯誤，依當輪狀態接續。
4. 對候選用 `report_read`，核對行情與實際設定。用 `loop_score` 傳 `loop_id`、`round`、0 至 100 的 `score` 與 `review`。達條件用 `loop_finish` 記錄停止原因與排名。
5. 交付 `loop_id`、每輪 `run_id`、最佳原始碼、報表與評分依據。用 `report_read(include_source=true)` 可取回該輪實際原碼。以報表的證據範圍描述相容性。

專案連線與工具參數詳見 `docs/strategy-studio-mcp.md`。若現行工作階段沒有載入 MCP，可在已設定的本機 Codex 或 Claude Code 工作階段使用相同工具。MCP 查得到某商品，仍要確認該策略的實際任務使用同一資料庫及行情來源。
