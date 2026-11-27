# API 連線設定

在「策略研究工作台 → 設定 → 行情與交易 API」，左側列表依設定名稱、券商 API 及用途顯示現有設定，可篩選全部、行情或交易。按「新增 API」建立設定。要編輯已有設定，選取一筆後按右上方「修改設定」，完成後按「儲存設定」。

已儲存的敏感欄位顯示固定六個星號。按欄位旁的「顯示」可在本機查看已儲存或正在輸入的值，再按「隱藏」遮回。閱讀模式下也可切換顯示，欄位維持唯讀。切換設定或離開頁面會重新隱藏敏感值。

凱基 QuoteCom 行情使用 `kgi_quotecom`，畫面提供使用者（UserID）、密碼（PassWord）、連線 IP 或主機（Host）、連接埠（Port）、來源（SourceID）及權杖（Token）。連線識別預設取設定名稱，舊設定中的固定連線名稱仍可沿用。

凱基 TradeCom 可由 `TradeCom設定.cmd` 開啟，或在 API 頁選「交易 → 新增 API → 凱基 TradeCom」。表單提供使用者、密碼、主機、連接埠、交易帳號、交易子帳及券商代碼。儲存後可於[交易工作台](trading.md)選用。MCP 使用 `kgi_tradecom` 和 `trade`。使用者放在 `credentials.account`，交易帳號與子帳分別放在 `credentials.secrets.trade_account` 與 `credentials.secrets.sub_account`，券商代碼放在 `profile.options.broker_id`。交易帳號、子帳及券商代碼以字串保存前導零。

其他券商可選「自訂券商 API」，填寫供應商代碼，再按券商文件新增公開或敏感的名稱和值。CA 憑證欄位保存本機絕對路徑，可用「選取憑證」開啟 Windows 選檔視窗。實際連線取決於對應轉接器，QuoteCom 行情目前已有轉接器。

帳號、密碼、權杖、CA 密碼及自訂敏感值由目前 Windows 使用者的 DPAPI 加密，與其他欄位一同寫入專案的 `data/api-profiles.sqlite`。編輯時留空會保留原憑證，勾選「清除」可移除該欄位。變更同一設定的供應商或用途時，請重新輸入欲使用的憑證。工具回應傳回一般欄位、憑證設定狀態與已儲存的自訂敏感欄位名稱。

## MCP

- `api_profile_schema` 接收 `provider` 與 `purpose`，回傳欄位代碼、顯示名稱、型別、連線必填狀態、自訂欄位支援及轉接器狀態。
- `api_profile_list` 列出工作台共用的設定和憑證狀態。
- `api_profile_save` 依 `profile.id` 新增或儲存設定。送出完整的公開連線欄位，自訂公開參數放在 `profile.options`。原有 `account`（UserID）、`password`、`token` 及 `ca_password` 欄位仍放在 `credentials`，自訂敏感參數放在 `credentials.secrets`，兩個自訂欄位物件均使用字串值。省略敏感欄位會保留同一供應商及用途的值，設定空字串則清除。設定可先儲存，QuoteCom 啟動時檢查六項連線必填值。正式憑證請由 UI 輸入，避免出現在 MCP 客戶端的呼叫紀錄。

```json
{
  "profile": {
    "id": "kgi-market",
    "provider": "kgi_quotecom",
    "purpose": "quote",
    "host": "quote.example.invalid",
    "port": 12345,
    "source_id": ""
  }
}
```

## QuoteCom 啟動

從專案目錄執行，並將範例參數替換成券商端點實測設定及實際契約。

```powershell
.\quote-live.ps1 -ProfileId KGIFutures -TradeType 020 -TimeDecimals 2 -PriceEncoding decoded -Instrument 'TXFL6=TXFL6','MXFL6=MXFL6','TMFL6=TMFL6'
```

此範例使用 2026/11/23 在 `KGIFutures` 端點實測的成交格式與當時的 12 月契約。其他端點須核對時間及價格格式，換月時須更新契約全碼。啟動程式從本機設定庫讀取資料，再將 QuoteCom 所需欄位交給轉接程序。時間小數位、價格編碼及商品對照詳見 [即時行情轉接與入庫](quote-gateway-protocol.md)。
