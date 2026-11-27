# 策略研究工作台

整合 PowerLanguage 策略編輯、行情管理、本機回測、AI 優化與 MCP。Windows 桌面介面使用 Godot，行情與回測核心使用 Rust。

## 原始碼與執行環境

- `rust/` 是行情、回測、MCP 與桌面橋接程式。
- `godot-workbench/` 是桌面介面，`native/` 是券商轉接程式。
- `docs/strategy-studio-mcp.md` 說明 MCP 工具及連線。
- 建置核心可使用 `cargo build --manifest-path rust/Cargo.toml --release --lib`。
- 建置命令列工具可使用 `cargo build --manifest-path rust/Cargo.toml --bins`。
- 桌面啟動器預期已安裝 Windows C++ 建置工具、Rust 及本機 Godot 4.7.2。現有啟動器使用 `.tools/` 內的專案工具配置。報表另外使用 Node.js 和 Python。
- 行情、券商元件與連線憑證需要在本機配置。本儲存庫提供原始碼與測試範例。

## MCP

完成建置後，在用戶端設定本機 `rust/target/debug/strategy_mcp.exe` 的絕對路徑。詳細設定見 [MCP 使用說明](docs/strategy-studio-mcp.md)。
