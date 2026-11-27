# 技術指標選用與公式

下列公式是常見定義。具體比較應使用同一平台、頻率、平滑及初始化設定。這些例子用來建立可檢驗的交易假設。

| 家族 | 指標與輸入 | 可測用途 | 常見陷阱 |
| --- | --- | --- | --- |
| 均線與平滑 | SMA、EMA、WMA、RMA、DEMA、TEMA、HMA、KAMA、VIDYA、ALMA、FRAMA、T3、ZLEMA | 趨勢方向、斜率、價格與均線距離、快慢線交叉 | 平滑的延遲與反應速度互相取捨。EMA 與 RMA 的係數、初始化及暖機可能不同 |
| 趨勢與盤勢 | DMI、ADX／ADXR、Aroon、Vortex、Parabolic SAR、一目均衡表、Supertrend、VHF、Choppiness、Kaufman ER | 趨勢強弱篩選、追蹤停損、避免盤整時追價 | ADX 測趨勢強度，方向另看 +DI／−DI。Supertrend 的帶狀更新與翻轉條件須逐棒確認 |
| 突破與支撐壓力 | Donchian、開盤區間、前日高低、Pivot Points、Fibonacci 回撤、Zig Zag、Williams Fractal、Swing High／Low | 進場觸價、假突破排除、停損位置 | 確認轉折需要右側資料，訊號要延後至確認棒。事後標出波段起點只適合解說 |
| 價格動能 | RSI、KD／Stochastic、Stoch RSI、Williams %R、ROC、Momentum、CCI、MACD／PPO、CMO、TSI、TRIX、Ultimate Oscillator、KST、AO、Fisher Transform | 順勢確認、區間反轉、加減速 | RSI 高低門檻只描述當前動能，強趨勢可長時間維持極端值。背離須在轉折確認後判定 |
| 波動與通道 | TR、ATR、NATR、ADR、布林通道、%B、BandWidth、Keltner、Chandelier Exit、標準差、歷史波動率、Parkinson／Garman–Klass／Rogers–Satchell | 波動篩選、停損距離、部位尺度、壓縮後突破 | ATR 是價格單位。年化波動率要交代報酬頻率及年化因子。跳空與夜盤的處理會影響估計 |
| 成交量與量價 | OBV、A/D Line、CMF、MFI、PVT、PVO、Force Index、Klinger、RVOL、VWMA、VWAP、Anchored VWAP、Volume Profile | 突破確認、盤中成交重心、相同時段的活躍度 | 價量近似指標與真實主動成交方向有不同資料需求。詳見期貨與市場資料技能 |
| 循環與濾波 | Hilbert 系列、MAMA／FAMA、帶通、Butterworth、高斯濾波、DPO | 區分週期性盤勢與趨勢，測試平滑或去趨勢 | 即時實作須為因果濾波，置中版本或全樣本濾波會使用未來資料。週期估計對缺棒敏感 |
| 價格轉換與圖表 | OHLC4、HLC3、Heikin-Ashi、Renko、Kagi、點數圖、Range bars | 建構特徵、降噪、訊號取樣 | 合成棒的開高低收無法直接代表真實委託成交價。先明確定義真實行情與訊號的時間對應 |
| K 棒及圖形型態 | 十字線、錘線、吞噬、晨星／夜星、三白兵／三黑鴉、頭肩、雙頂／雙底、旗形、三角整理 | 作為有明確確認時點的條件 | 實體、影線、缺口、確認棒與趨勢背景應量化。TA-Lib 型態回傳代碼需查函式定義 |
| 統計與相對強弱 | 滾動標準差、Z 分數、百分位、線性迴歸斜率、相關、Beta、偏態、峰度、配對價差 | 衡量偏離、趨勢、跨商品對照與風險曝險 | 指定價格或報酬序列、母體或樣本標準差、百分位方法、零分母及時間對齊。相對強弱比值與 RSI 的意義不同 |

資料與公式索引。 [TA-Lib 官方函式](https://ta-lib.org/functions/) · [TradingView 指標清單](https://www.tradingview.com/charting-library-docs/latest/ui_elements/indicators/Indicators-List/) · [TradingView Supertrend](https://www.tradingview.com/support/solutions/43000634738-supertrend/) · [TradingView 重新繪製說明](https://tradingview.com/pine-script-docs/concepts/repainting/)

## 常用公式與回測差異

- **SMA／EMA／RMA**。SMA 是最近 N 筆的算術平均。EMA 通常以 `2/(N+1)` 更新，Wilder RMA 以 `1/N` 更新。先核對首值是第一筆還是前 N 筆平均，與缺值時是否跳過更新。 [TA-Lib 公式目錄](https://ta-lib.org/functions/)
- **RSI 與 KD**。Wilder RSI 常見為 `100 − 100/(1+平均漲幅/平均跌幅)`，兩者以 Wilder 平滑。KD 的原始 %K 常用 `100×(C−區間最低)/(區間最高−區間最低)`，%D 是其平滑序列。快版、慢版、臺灣常見遞迴 KD 及 KDJ 應分別核對；全平、無跌或高低相同時要以平台規則驗證。 [TA-Lib 公式目錄](https://ta-lib.org/functions/)
- **MACD、PPO、ROC**。MACD 常見為快慢 EMA 之差，訊號線再對差值平滑，柱狀為兩者差值。PPO 將快慢均線差除以慢均線後乘 100。ROC 常用 `100×(C/C[N]−1)`。有些庫提供比例或以 100 為中線的版本。 [TA-Lib 公式目錄](https://ta-lib.org/functions/)
- **TR、ATR、NATR**。TR 是當棒高低差、最高價與前收絕對差、最低價與前收絕對差三者最大值。Wilder ATR 對 TR 用 Wilder 平滑。以 `100×ATR/Close` 為常見 NATR 定義，使用前仍須核對函式的第一棒與分母規則。 [TA-Lib 公式目錄](https://ta-lib.org/functions/)
- **布林、Keltner、Donchian**。布林通道常以 SMA 加減 K 倍標準差，確認分母、%B 與 BandWidth 定義。碰上軌只表示相對位置，需有獨立的交易假設。現代 Keltner 常用 EMA 加減 ATR 倍數，其他版本可能使用不同價格與區間。Donchian 取區間最高和最低，若以收盤突破比較，常需明確排除當棒，例如拿當棒收盤比較前一棒已知的通道。 [John Bollinger 使用原則](https://www.bollingerbands.com/bollinger-band-rules) · [TA-Lib 公式目錄](https://ta-lib.org/functions/)
- **OBV、RVOL、VWAP**。OBV 依收盤較前收漲跌加減成交量。RVOL 要交代分母是一般滾動平均或歷史同時段平均。VWAP 用 `Σ(價×量)/Σ量`，逐筆成交價與 K 棒代表價會得出不同結果，需交代錨點、日夜盤重置和無成交情形。 [TA-Lib 公式目錄](https://ta-lib.org/functions/)
