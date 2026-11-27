extends "res://skin.gd"

const Workbench = preload("res://main.tscn")
const Quote = preload("res://quote/main.tscn")
const ApiProfiles = preload("res://api_profiles.gd")
const Chat = preload("res://chat_home.gd")
const Settings = preload("res://workspace_settings.gd")
const Telegram = preload("res://telegram_bot.gd")
const StrategyExecution = preload("res://strategy_execution.gd")
const LiveChart = preload("res://live_chart.gd")
const TradeReports = preload("res://trade_reports.gd")
enum { HOME, MARKET, STRATEGIES, SETTINGS, EXECUTION, LIVE_CHART, REPORTS, CODE, BACKTEST, OPTIMIZE, VERSIONS }
enum { SETTINGS_AI, SETTINGS_BROKER, SETTINGS_TELEGRAM }

var workbench: Control
var quote: Control
var api_profiles: Control
var navigation: TabBar
var chat: Control
var settings: Control
var settings_page: VBoxContainer
var settings_sections: TabBar
var settings_content: Control
var telegram: Node
var strategy_execution: Control
var live_chart: Control
var trade_reports: Control
var connection_bridge: RefCounted
var quote_light: Label
var trade_light: Label

func _ready() -> void:
	get_tree().auto_accept_quit=false
	_theme()
	var layout := VBoxContainer.new()
	layout.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(layout)
	var header := _row(layout)
	var brand := _label(header,"  策略研究工作台",19,TEAL)
	brand.size_flags_vertical=SIZE_SHRINK_CENTER
	navigation=TabBar.new()
	navigation.size_flags_horizontal=SIZE_EXPAND_FILL
	for title in ["對話首頁","行情資料庫","策略工作區","設定","策略執行","即時 K 線","交易回報"]: navigation.add_tab(title)
	header.add_child(navigation)
	quote_light=_label(header,"● 行情 未連線",13,MUTED)
	trade_light=_label(header,"● 交易 未連線",13,MUTED)
	for light in [quote_light,trade_light]:
		light.size_flags_vertical=SIZE_SHRINK_CENTER
		light.mouse_filter=Control.MOUSE_FILTER_PASS
	if ClassDB.class_exists("ApiConnectionBridge"):
		connection_bridge=ClassDB.instantiate("ApiConnectionBridge")
		var timer := Timer.new()
		timer.wait_time=1.0
		timer.timeout.connect(_poll_connections)
		add_child(timer)
		timer.start()
		_poll_connections()
	var content := Control.new()
	content.size_flags_vertical=SIZE_EXPAND_FILL
	layout.add_child(content)
	workbench=Workbench.instantiate()
	workbench.embedded=true
	content.add_child(workbench)
	settings_page=VBoxContainer.new()
	settings_page.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	settings_page.hide()
	content.add_child(settings_page)
	settings_sections=TabBar.new()
	for title in ["AI API","行情與交易 API","Telegram"]: settings_sections.add_tab(title)
	settings_page.add_child(settings_sections)
	settings_content=Control.new()
	settings_content.size_flags_vertical=SIZE_EXPAND_FILL
	settings_page.add_child(settings_content)
	settings=Settings.new()
	settings_content.add_child(settings)
	settings_sections.tab_changed.connect(_show_settings_section)
	workbench.optimizer.settings_loader=settings.ai_config
	workbench.optimizer.settings_requested.connect(func(): open_settings(SETTINGS_AI))
	chat=Chat.new()
	chat.studio=self
	content.add_child(chat)
	workbench.optimizer.review_requested.connect(chat.review_optimization)
	settings.saved.connect(chat.refresh_models)
	telegram=Telegram.new()
	telegram.config_loader=settings.telegram_config
	add_child(telegram)
	telegram.prompt_received.connect(chat.submit_telegram)
	telegram.state_changed.connect(settings.update_telegram_state)
	settings.telegram_start_requested.connect(func(test: bool): telegram.start(settings.telegram_config(),test))
	settings.telegram_stop_requested.connect(func(): telegram.stop())
	settings.saved.connect(func(): telegram.settings_changed(settings.telegram_config()))
	var launch_args := OS.get_cmdline_user_args()
	var initial_tab := HOME
	if ["--open-settings","--open-mixroute-key","--open-api-settings","--open-tradecom-settings"].any(func(arg): return arg in launch_args):
		initial_tab=SETTINGS
	elif "--open-quote" in launch_args:
		initial_tab=MARKET
	navigation.current_tab=initial_tab
	navigation.tab_changed.connect(_navigate)
	workbench.operation_finished.connect(chat.workbench_finished)
	_navigate(navigation.current_tab)
	if "--open-tradecom-settings" in launch_args:
		open_settings(SETTINGS_BROKER,true)
	elif "--open-api-settings" in launch_args:
		open_settings(SETTINGS_BROKER)
	elif "--open-mixroute-key" in launch_args:
		open_settings(SETTINGS_AI)
		settings.open_provider_key("mixroute")

func open_page(page: int) -> void:
	match page:
		CODE, BACKTEST, OPTIMIZE, VERSIONS:
			navigation.current_tab=STRATEGIES
			match page:
				CODE: workbench.pages.current_tab=workbench.EDIT_PAGE
				BACKTEST: workbench.pages.current_tab=workbench.BACKTEST_PAGE
				OPTIMIZE: workbench.pages.current_tab=workbench.OPTIMIZE_PAGE
				VERSIONS: workbench.pages.current_tab=workbench.VERSION_PAGE
		_:
			navigation.current_tab=page

func _poll_connections() -> void:
	var rows: Variant=JSON.parse_string(connection_bridge.snapshot())
	if not rows is Array: return
	_show_connection(quote_light,"行情","quote",rows)
	_show_connection(trade_light,"交易","trade",rows)

func _show_connection(light: Label, title: String, kind: String, rows: Array) -> void:
	var states := {"ready":"已連線","connecting":"連線中","reconnecting":"重連中","offline":"中斷"}
	var details := PackedStringArray()
	var ready := 0
	var waiting := false
	var reconnecting := false
	var failed := false
	for row in rows:
		if row.kind!=kind: continue
		var state: String=row.state
		details.append(str(row.label)+" · "+str(states.get(state,"未連線")))
		ready+=int(state=="ready")
		waiting=waiting or state in ["connecting","reconnecting"]
		reconnecting=reconnecting or state=="reconnecting"
		failed=failed or state=="offline"
	var description := "未連線"
	var color: Color=MUTED
	if ready>0 and ready==details.size():
		description="已連線"
		color=TEAL
	elif ready>0 or waiting:
		description="部分連線" if ready>0 else ("重連中" if reconnecting else "連線中")
		color=Color("#f4c266")
	elif failed:
		description="中斷"
		color=Color("#ff788d")
	light.text="● "+title+" "+description
	light.add_theme_color_override("font_color",color)
	light.tooltip_text="\n".join(details) if not details.is_empty() else title+"服務目前未連線。"
	light.tooltip_text+="\n每天臺灣時間 06:00 重連。"

func open_settings(section: int=SETTINGS_AI, tradecom := false) -> void:
	settings_sections.current_tab=section
	navigation.current_tab=SETTINGS
	if tradecom and api_profiles!=null: api_profiles.open_tradecom()

func _show_settings_section(section: int) -> void:
	if section==SETTINGS_BROKER and api_profiles==null:
		api_profiles=ApiProfiles.new()
		settings_content.add_child(api_profiles)
	if api_profiles!=null: api_profiles.visible=section==SETTINGS_BROKER
	if section!=SETTINGS_BROKER: settings.show_section(section==SETTINGS_TELEGRAM)
	settings.visible=section!=SETTINGS_BROKER

func _navigate(tab: int) -> void:
	chat.visible=tab==HOME
	settings_page.visible=tab==SETTINGS
	if chat.visible:
		chat.composer.grab_focus()
	workbench.visible=tab==STRATEGIES
	if tab==MARKET:
		ensure_quote()
	if quote!=null: quote.visible=tab==MARKET
	if tab==EXECUTION and strategy_execution==null:
		strategy_execution=StrategyExecution.new()
		strategy_execution.workbench=workbench
		strategy_execution.reports_requested.connect(_open_trade_reports)
		workbench.get_parent().add_child(strategy_execution)
	if strategy_execution!=null:
		strategy_execution.visible=tab==EXECUTION
		if strategy_execution.visible: strategy_execution.reload()
	if tab==LIVE_CHART and live_chart==null:
		live_chart=LiveChart.new()
		live_chart.workbench=workbench
		live_chart.new_strategy_requested.connect(func():
			open_page(CODE)
			workbench.new_strategy())
		workbench.get_parent().add_child(live_chart)
	if live_chart!=null:
		live_chart.visible=tab==LIVE_CHART
		if live_chart.visible: live_chart.reload_profiles()
	if tab==REPORTS and trade_reports==null:
		trade_reports=TradeReports.new()
		workbench.get_parent().add_child(trade_reports)
	if trade_reports!=null: trade_reports.visible=tab==REPORTS

func _open_trade_reports(profile: String) -> void:
	navigation.current_tab=REPORTS
	trade_reports.focus_profile(profile)

func ensure_quote() -> void:
	if quote==null:
		quote=Quote.instantiate()
		quote.embedded=true
		quote.hide()
		quote.market_selected.connect(_market_selected)
		quote.operation_finished.connect(chat.market_finished)
		workbench.get_parent().add_child(quote)

func _market_selected(db: String, instrument: Dictionary, source_kind: int) -> void:
	if workbench.busy or workbench.optimizer.running or workbench.active<0:
		quote._error("策略工作台正在執行作業，請稍候再帶入商品。")
		return
	open_page(BACKTEST)
	workbench.select_market(db,instrument,source_kind)

func _notification(what: int) -> void:
	if what!=NOTIFICATION_WM_CLOSE_REQUEST: return
	if strategy_execution!=null and strategy_execution.running:
		navigation.current_tab=EXECUTION
		strategy_execution.stop()
		return
	if quote!=null and quote.busy:
		navigation.current_tab=MARKET
		quote._error("行情資料庫正在執行作業，請等完成後關閉。")
		return
	if workbench.busy or workbench.optimizer.running:
		if workbench.optimizer.running: open_page(OPTIMIZE)
		else: navigation.current_tab=STRATEGIES
	if not workbench.prepare_close(): return
	if quote!=null and quote.dirty:
		navigation.current_tab=MARKET
		quote._ask("商品有尚未儲存的修改，確定放棄並關閉？",func(): get_tree().quit())
	else: get_tree().quit()
