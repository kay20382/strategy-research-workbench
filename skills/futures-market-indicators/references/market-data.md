# 期貨與市場指標的資料需求

| 家族 | 指標範例 | 最低資料及關鍵定義 | 優化假設 |
| --- | --- | --- | --- |
| 量價 | OBV、A/D Line、CMF、MFI、PVT、RVOL、VWMA | 有實際量的 OHLCV，需區分成交口數、成交筆數與報價 Tick 次數 | 突破需有相對活躍的成交量，並按日夜盤或盤內同時段比較 |
| 盤中成交重心 | VWAP、Anchored VWAP、VWAP 標準差帶 | 逐筆價量或明確標示的棒級近似，錨點須在當時已可決定 | 用當盤累計成交重心篩選追價，或用前盤已確認重心做價位條件 |
| 價位分布 | Volume Profile、POC、VAH／VAL、HVN／LVN、TPO | Volume Profile 以價位的成交量分布；TPO 按時間段到訪價位計數。定義價格分箱、區間、價值區算法與同量時取法 | 用前盤已完成價位分布找突破或回測支撐壓力，另測盤中逐步更新版 |
| 成交流與流動性 | Bid／Ask Delta、CVD、Footprint、買賣價差、深度、委託簿不平衡、Order Flow Imbalance | 真實主動方向需來源標記或可驗證的分類規則；委託簿需要同時點的買賣報價與數量，事件型 OFI 還需要增減、成交及撤單順序 | 測試盤面深度是否影響進場、滑價或出場觸發。對照方向缺漏、同時間戳與資料斷線 |
| 未平倉與交易人部位 | OI、OI 變化、法人淨部位、大額交易人集中度、COT | 按商品、月份、盤別及報表口徑對齊，保留發布時間及修訂。未平倉量每口只計一邊 | 將部位變化當作慢速條件，確認到期轉倉造成的變動是否扭曲訊號 |
| 選擇權情緒與波動 | Put／Call Ratio、隱含波動率、VIX、TAIWAN VIX、偏斜、期限結構、Greeks | 比率區分成交量與未平倉量。隱含波動率及 Greeks 需報價、履約價、到期、標的或遠期與模型假設 | 測試高波動環境對交易頻率、停損或部位的影響。波動率水準衡量幅度，方向要另有假設 |
| 期現貨與期限結構 | 基差、跨月價差、展期收益、正價差／逆價差、跨商品價差 | 同步期現貨或不同月份報價，寫明相減方向、契約乘數、貨幣、換月與調整方法 | 測試遠近月結構是否能辨識盤勢，並將策略訊號序列與可交易契約分開 |
| 市場廣度與相對表現 | 漲跌家數、A/D Line、TRIN、新高新低、均線以上比例、相對強弱比、相關與 Beta | 當時的樣本成分、個股行情及公司事件。定義停牌、無比價、股票與全市場口徑 | 用市場參與程度篩選指數突破，檢查存活者偏誤、現貨休盤及夜盤只見前次公開資料的情形 |
| 基本面、總經與供需 | 營收年增率、估值、利率曲線、通膨、庫存變動、報告意外值、季節性 | 公司揭露、總經或商品報告的原始發布值、當時市場預估、修訂版本與日期。季節性需定義季節與訓練區間 | 用當時已知資訊篩選事件風險與盤勢，避免用修訂後數據或全樣本季節平均回填歷史 |

成交量、未平倉、VIX 及報價欄位定義。 [CME 成交量與未平倉量](https://www.cmegroup.com/trading/about-volume.html) · [Cboe VIX FAQ](https://www.cboe.com/tradable_products/vix/faqs) · [TradingView 量價分布](https://www.tradingview.com/support/solutions/43000502040-volume-profile-indicators-basic-concepts/) · [Databento 方向欄位範例](https://databento.com/docs/datasets/ocea-memoir)

## 臺灣資料入口

- 臺指選擇權 Put／Call 表分列成交量比及未平倉量比。讀附註確認週到期與月到期涵蓋範圍，策略中保留兩個欄位的來源時間。 [臺灣期交所 Put／Call 比](https://www.taifex.com.tw/cht/3/pcRatio)
- TAIWAN VIX 使用臺指選擇權報價推算，並有自身揭示規則。若用日終值，依該欄位公布時點決定最早可進入策略的交易棒。 [臺灣期交所波動率指數說明](https://www.taifex.com.tw/cht/7/vixQA) · [臺指選擇權波動率指數下載](https://www.taifex.com.tw/cht/7/vixMinNew)
- 法人淨部位是分類後的合計值。夜盤資料要依交易歸屬日期及實際發布時間對齊。 [臺灣期交所夜盤三大法人](https://www.taifex.com.tw/cht/3/futContractsDateAhView) · [期貨交易資訊觀測站](https://www.taifex.com.tw/eventTaifexTradingCenter/cht/index.do)
- 廣度資料先選樣本範圍，並查無比價或公司事件排除口徑。 [證交所每日收盤行情](https://wwwc.twse.com.tw/zh/trading/historical/mi-index.html)
- 公司營收與財務報表以公開揭露時點為準。估值或成長率要對齊當時的財報版本、股本及還原口徑。 [公開資訊觀測站](https://mops.twse.com.tw/mops/)

## 海外資料與驗證

- COT 通常採週二持倉、週五公布，例外依實際發布行事曆。回測只能從發布後使用該筆。不同報表系列及交易人分類需分開。 [CFTC COT 報表](https://www.cftc.gov/MarketReports/CommitmentsofTraders/index.htm) · [CFTC 發布行事曆](https://www.cftc.gov/MarketReports/CommitmentsofTraders/ReleaseSchedule/index.htm)
- 美國總經序列可用 ALFRED 取得當時可見的版本，盤中事件仍需查精確發布時刻。能源庫存依 EIA 實際發布行事曆及欄位時間處理，遇假日查改期。 [ALFRED 資料版本說明](https://fred.stlouisfed.org/docs/api/fred/realtime_period.html) · [EIA 石油庫存發布行事曆](https://www.eia.gov/petroleum/supply/weekly/schedule.php)
- 只有成交價和量時，主動買賣方向可透過明示規則估計。要評估重複成交、價差內成交、同價成交與分類缺失，並保留原始 Tick 的順序。委託簿訊號要另外確認深度及事件資料。 [資料欄位範例](https://databento.com/docs/datasets/ocea-memoir)
- 原始資料跨來源時，保存取得版本、原時區、欄位單位、發布時間、交易所盤別、契約月份、合併方式及未覆蓋期間。回測比較時也應保存外部資料版本或雜湊。
