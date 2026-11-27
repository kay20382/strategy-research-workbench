---
name: strategy-optimization
description: 使用 策略研究工作台 MCP 讀取策略回測報表、根據績效問題挑選交易指標、修改 PowerLanguage、評分並迭代。用於策略候選比較、樣本外驗證或接續既有研究迴圈。
---

# 策略指標優化

1. 用 `strategy_list` 和 `strategy_read` 找原碼及實際任務。重名時核對已驗收原始碼、設定和舊報表。使用者提供 `loop_id` 時，先用 `loop_status` 接續。
2. 跑原版並用 `report_read` 核對行情、暖機、成本、盤別、撮合與報表。從損失集中在哪一側、時段、成交密度或出場方式推導假設。依需求讀 `trading-indicator-reference` 或 `futures-market-indicators`。優先測試能回答該問題的單一變更。
3. 在看到候選結果前定義評分及淘汰條件。依 [實驗與評分](references/experiment.md) 保存已試參數、成本、樣本內及樣本外區間。既有迴圈沿用原準則。若要改評分口徑，建立新的研究迴圈並說明。
4. 依 [工作台相容性與 MCP](references/workbench-mcp.md) 確認 PL 函式及資料支援。用 `strategy_check` 驗證候選，使用 `loop_submit` 以相同行情及設定回測，接著用 `report_read`、`backtest_result` 讀取證據並以 `loop_score` 記錄評語。
5. 完成指定輪數或達到停止條件時，用 `loop_finish` 保存結果。交付各輪程式碼與修改假設、實際績效、分數、報表、最佳版及其驗證範圍。若原版分數最高，最佳版為原版。

此技能支援研究與回測。任何實盤部署、參數套用到下單或策略庫覆寫，依使用者的個別指示處理。
