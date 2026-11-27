extends "res://skin.gd"

signal operation_finished(operation: String, data: Variant, error: String)

const Equity = preload("res://equity.gd")
const TradeReplay = preload("res://trade_replay.gd")
const DT = preload("res://date_format.gd")
const DatePicker = preload("res://date_picker.gd")
const Optimizer = preload("res://optimizer.gd")
const EDIT_PAGE := 0
const BACKTEST_PAGE := 1
const RESULT_PAGE := 2
const OPTIMIZE_PAGE := 3
const VERSION_PAGE := 4
var optimizer: VBoxContainer
var embedded := false
var root := ""
var bridge: RefCounted
var catalog: Array = []
var documents: Array = []
var active := -1
var busy := false
var loading := false
var action := ""
var elapsed := 0.0
var search: LineEdit
var library_list: ItemList
var library_kind: OptionButton
var code_search: LineEdit
var doc_tabs: TabBar
var pages: TabContainer
var workspace_tools: MenuButton
var report_menu: MenuButton
var editor: CodeEdit
var source_info: Label
var status: Label
var progress: ProgressBar
var output: RichTextLabel
var run_info: Label
var metrics: Dictionary = {}
var chart: Control
var trades: Tree
var trade_replay: Window
var trade_replay_button: Button
var robustness_dialog: ConfirmationDialog
var robustness_fields := {}
var robustness_report_path := ""
var version_confirm: ConfirmationDialog
var version_list: ItemList
var version_message: LineEdit
var version_remote: LineEdit
var version_branch: LineEdit
var version_status: Label
var version_source: CodeEdit
var version_parameters: CodeEdit
var version_document_key := ""
var version_pending_refresh := false
var page_info: Label
var trade_page := 0
var trade_count := 0
var fields: Dictionary = {}
var unit: OptionButton
var pyramid: OptionButton
var inputs: CodeEdit
var advanced: CodeEdit
var excel: CheckBox
var destination: LineEdit
var file_dialog: FileDialog
var file_action := ""
var notice: AcceptDialog
var confirm: ConfirmationDialog
var confirm_callback: Callable
var last_view: Dictionary = {}
var smoke := false
var smoke_dir := ""
var smoke_stage := 0
var first_net := 0.0
var smoke_excel := false
var smoke_receipts: Array = []
var session_timer := 0.0
var last_session_text := ""

func _ready() -> void:
	root=ProjectSettings.globalize_path("res://..").simplify_path()
	for arg in OS.get_cmdline_user_args():
		if arg.begins_with("--smoke-directory="): smoke=true; smoke_dir=arg.trim_prefix("--smoke-directory=")
		if arg=="--smoke-excel": smoke_excel=true
	get_tree().auto_accept_quit=false
	_theme()
	theme.set_stylebox("panel","Tree",_style(Color("0b1423")))
	theme.set_color("font_color","Tree",INK)
	for cls in ["CodeEdit","TextEdit"]:
		theme.set_stylebox("normal",cls,_style(Color("0b1423")))
		theme.set_stylebox("focus",cls,_style(Color("0b1423"),Color("3c727c")))
		theme.set_color("font_color",cls,INK)
		theme.set_color("line_number_color",cls,MUTED)
		theme.set_color("caret_color",cls,TEAL)
		theme.set_color("selection_color",cls,Color("305775"))
	_build()
	if not ClassDB.class_exists("WorkbenchBridge"): _error("Rust模組尚未載入，請執行 pl-workbench-godot.ps1 -Build"); return
	bridge=ClassDB.instantiate("WorkbenchBridge")
	_request("init")
	modulate.a=0
	create_tween().tween_property(self,"modulate:a",1.0,0.35)

func _build() -> void:
	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	for side in ["left","right","top","bottom"]: margin.add_theme_constant_override("margin_"+side,16)
	add_child(margin)
	var main := VBoxContainer.new()
	main.add_theme_constant_override("separation",8)
	margin.add_child(main)
	var body := HSplitContainer.new()
	body.size_flags_vertical=SIZE_EXPAND_FILL
	main.add_child(body)
	var nav := _panel(body)
	var sidebar := nav.get_parent() as PanelContainer
	sidebar.size_flags_horizontal=SIZE_FILL
	nav.add_theme_constant_override("separation",8)
	var library_header := _row(nav)
	var library_title := _label(library_header,"策略庫",16)
	library_title.size_flags_horizontal=SIZE_EXPAND_FILL
	var library_menu := _menu(library_header,"新增 ▾",[
		["新增策略",new_strategy],
		["匯入 PL…",func(): _choose("import")],
	])
	library_menu.tooltip_text="新增或匯入策略"
	search=LineEdit.new()
	search.placeholder_text="搜尋名稱"
	search.text_changed.connect(func(_s): _catalog_list())
	nav.add_child(search)
	library_kind=OptionButton.new()
	for title in ["策略","函式","全部"]: library_kind.add_item(title)
	library_kind.item_selected.connect(func(_i): _catalog_list())
	nav.add_child(library_kind)
	library_list=ItemList.new()
	library_list.custom_minimum_size.x=224
	library_list.size_flags_vertical=SIZE_EXPAND_FILL
	library_list.item_activated.connect(func(index): _checkpoint(); _request("open",{"entry":library_list.get_item_metadata(index)}))
	nav.add_child(library_list)
	library_list.tooltip_text="雙擊開啟策略或函式"
	var workspace := VBoxContainer.new()
	workspace.size_flags_horizontal=SIZE_EXPAND_FILL
	workspace.add_theme_constant_override("separation",6)
	body.add_child(workspace)
	var documents_bar := _row(workspace)
	var library_toggle := Button.new()
	library_toggle.text="策略庫"
	library_toggle.toggle_mode=true
	library_toggle.button_pressed=true
	library_toggle.tooltip_text="顯示或收起左側策略庫"
	library_toggle.toggled.connect(func(shown: bool): sidebar.visible=shown)
	documents_bar.add_child(library_toggle)
	doc_tabs=TabBar.new()
	doc_tabs.size_flags_horizontal=SIZE_EXPAND_FILL
	doc_tabs.tab_close_display_policy=TabBar.CLOSE_BUTTON_SHOW_ACTIVE_ONLY
	doc_tabs.tab_changed.connect(_activate)
	doc_tabs.tab_close_pressed.connect(_close_document)
	documents_bar.add_child(doc_tabs)
	_button(documents_bar,"儲存",func(): _document_action("save")).tooltip_text="儲存策略副本（Ctrl+S）"
	_button(documents_bar,"▶ 開始回測",_run,true).tooltip_text="開始回測（F5）"
	var toolbar := _row(workspace)
	var page_buttons: Array[Button]=[]
	for label in ["程式碼","回測設定","回測結果"]:
		var page := page_buttons.size()
		var selector := Button.new()
		selector.text=label
		selector.toggle_mode=true
		selector.pressed.connect(func(): _select_workspace_page(page,page_buttons))
		toolbar.add_child(selector)
		page_buttons.append(selector)
	source_info=_label(toolbar,"尚未開啟策略",12,MUTED)
	source_info.size_flags_horizontal=SIZE_EXPAND_FILL
	source_info.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS
	workspace_tools=_menu(toolbar,"工具 ▾",[
		["搜尋程式碼（Ctrl+F）",_show_code_search],
		["檢查語法（F7）",func(): _document_action("check")],
		["邏輯檢查與流程圖",func(): _document_action("strategy_logic")],
		["讀入回測設定…",func(): _choose("load_task")],
		["AI 優化",func(): pages.current_tab=OPTIMIZE_PAGE],
		["版本控制",func(): pages.current_tab=VERSION_PAGE],
		["匯出 PowerLanguage（CP950）",func(): _document_action("strategy_export_pl")],
	])
	pages=TabContainer.new()
	pages.tabs_visible=false
	pages.size_flags_vertical=SIZE_EXPAND_FILL
	pages.use_hidden_tabs_for_min_size=false
	workspace.add_child(pages)
	var code_page := VBoxContainer.new()
	code_page.name="PL 程式碼"
	pages.add_child(code_page)
	var find_row := _row(code_page)
	find_row.hide()
	code_search=LineEdit.new()
	code_search.placeholder_text="搜尋程式碼（Ctrl+F）"
	code_search.size_flags_horizontal=SIZE_EXPAND_FILL
	code_search.text_submitted.connect(func(_t): _find_code())
	find_row.add_child(code_search)
	_button(find_row,"尋找下一個",_find_code)
	_button(find_row,"關閉",func(): find_row.hide(); editor.grab_focus())
	editor=_code_edit()
	editor.size_flags_vertical=SIZE_EXPAND_FILL
	editor.gutters_draw_line_numbers=true
	editor.minimap_draw=true
	editor.minimap_width=100
	editor.indent_use_spaces=true
	editor.indent_size=4
	editor.text_changed.connect(_source_changed)
	code_page.add_child(editor)
	var settings_scroll := ScrollContainer.new()
	settings_scroll.name="回測設定"
	pages.add_child(settings_scroll)
	var settings := VBoxContainer.new()
	settings.size_flags_horizontal=SIZE_EXPAND_FILL
	settings.add_theme_constant_override("separation",10)
	settings_scroll.add_child(settings)
	var identity := _row(settings)
	fields.strategy_name=_field(identity,"策略名稱")
	fields.symbol=_field(identity,"商品代碼")
	fields.database=_field(identity,"資料庫路徑")
	var market := _row(settings)
	unit=OptionButton.new()
	for s in ["沿用任務來源","秒 K（Tick）","分 K（1分K）","日 K（日K）"]: unit.add_item(s)
	market.add_child(unit)
	fields.period=_field(market,"K棒週期")
	fields.warmup_bars=_field(market,"MaxBarsBack 暖機棒數")
	fields.contracts=_field(market,"預設委託口數")
	var dates := _row(settings)
	fields.from=DatePicker.labeled(dates,"起始時間（空白＝現有第一筆，之後暖機）","",true)
	fields.to=DatePicker.labeled(dates,"結束時間","",true)
	fields.to.default_time="23:59:59"
	var cost := _row(settings)
	for pair in [["initial_capital","原始資金"],["commission_per_side","單邊每口手續費"],["slippage_ticks","滑價跳動數"],["point_value","每點價值"]]: fields[pair[0]]=_field(cost,pair[1])
	var scale := _row(settings)
	pyramid=OptionButton.new()
	for s in ["不允許加碼","無論是否相同訊號","排除相同訊號（尚待驗證）"]: pyramid.add_item(s)
	scale.add_child(pyramid)
	fields.max_position_contracts=_field(scale,"最多持倉口數（空白沿用）")
	fields.max_entry_batches=_field(scale,"最多進場批數（空白沿用）")
	_label(settings,"策略輸入參數（JSON）",13,TEAL)
	inputs=_code_edit()
	inputs.custom_minimum_size.y=100
	settings.add_child(inputs)
	var advanced_heading := _row(settings)
	var advanced_settings := VBoxContainer.new()
	var advanced_toggle := Button.new()
	advanced_toggle.text="完整任務 JSON ▾"
	advanced_toggle.toggle_mode=true
	advanced_toggle.toggled.connect(func(shown: bool): advanced_settings.visible=shown)
	advanced_heading.add_child(advanced_toggle)
	settings.add_child(advanced_settings)
	advanced_settings.hide()
	advanced=_code_edit()
	advanced.custom_minimum_size.y=220
	advanced_settings.add_child(advanced)
	_button(advanced_settings,"套用完整任務 JSON",_apply_advanced)
	var reports := _row(settings)
	excel=CheckBox.new()
	excel.text="產出 Excel"
	excel.size_flags_vertical=SIZE_SHRINK_CENTER
	excel.tooltip_text="回測後匯出Excel，並執行報表驗證"
	excel.button_pressed=bool(ProjectSettings.get_setting("rea_workbench/excel_default",true))
	reports.add_child(excel)
	destination=_field(reports,"Excel報告資料夾（空白沿用設定）")
	_label(settings,"回測使用本機 PL 相容引擎，支援範圍依已實作的語法與函式。",12,MUTED)
	for f in fields.values(): f.text_changed.connect(func(_t): _dirty())
	inputs.text_changed.connect(_dirty)
	unit.item_selected.connect(func(_i): _dirty())
	pyramid.item_selected.connect(func(_i): _dirty())
	var results := VBoxContainer.new()
	results.name="回測結果與報表"
	results.add_theme_constant_override("separation",8)
	pages.add_child(results)
	var cards := _row(results)
	cards.add_theme_constant_override("separation",20)
	for pair in [["net_closed","已平倉淨利"],["max_drawdown","最大回撤"],["trades","已平倉交易"],["win_rate","勝率"]]:
		var box := VBoxContainer.new()
		box.size_flags_horizontal=SIZE_EXPAND_FILL
		cards.add_child(box)
		_label(box,pair[1],12,MUTED)
		metrics[pair[0]]=_label(box,"—",22,TEAL)
	run_info=_label(results,"尚未執行回測；結果會綁定本次程式碼與設定快照。",12,MUTED)
	run_info.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	var result_split := VSplitContainer.new()
	result_split.size_flags_vertical=SIZE_EXPAND_FILL
	results.add_child(result_split)
	chart=Equity.new()
	chart.size_flags_vertical=SIZE_EXPAND_FILL
	result_split.add_child(chart)
	var trade_section := VBoxContainer.new()
	trade_section.size_flags_vertical=SIZE_EXPAND_FILL
	trade_section.size_flags_stretch_ratio=1.3
	result_split.add_child(trade_section)
	var report_tools := _row(trade_section)
	_label(report_tools,"交易明細",15)
	_button(report_tools,"‹",func(): _trade_page(-1)).tooltip_text="上一頁交易"
	page_info=_label(report_tools,"尚未產生",12,MUTED)
	_button(report_tools,"›",func(): _trade_page(1)).tooltip_text="下一頁交易"
	var report_spacer := Control.new()
	report_spacer.size_flags_horizontal=SIZE_EXPAND_FILL
	report_tools.add_child(report_spacer)
	trade_replay_button=_button(report_tools,"K 線回放",_open_trade_replay)
	trade_replay_button.disabled=true
	report_menu=_menu(report_tools,"報表與檢測 ▾",[
		["開啟 HTML 報表",func(): _open_artifact("html")],
		["開啟 Excel",func(): _open_artifact("excel")],
		["本次輸出資料夾",func(): _open_artifact("folder")],
		["策略強健性檢測",_open_robustness],
	])
	report_menu.disabled=true
	trades=Tree.new()
	trades.size_flags_vertical=SIZE_EXPAND_FILL
	trades.custom_minimum_size.y=100
	trades.hide_root=true
	trades.columns=9
	trades.column_titles_visible=true
	for i in range(9): trades.set_column_title(i,["序號","方向／口數","進場時間","出場時間","進場價","出場價","淨損益","累計損益","進場／出場訊號"][i])
	trades.set_column_custom_minimum_width(0,45)
	trades.set_column_expand(0,false)
	for i in [2,3]: trades.set_column_custom_minimum_width(i,145)
	trades.set_column_custom_minimum_width(8,170)
	trades.item_activated.connect(func():
		var row := trades.get_selected()
		if row!=null: _open_trade_replay(row.get_metadata(0)))
	trade_section.add_child(trades)
	if optimizer==null: optimizer=Optimizer.new()
	pages.add_child(optimizer)
	optimizer.build(self)
	optimizer.requested.connect(_request)
	optimizer.running_changed.connect(func(_running): _set_busy(busy))
	var line := _row(main)
	status=_label(line,"準備就緒",13,TEAL)
	status.size_flags_horizontal=SIZE_EXPAND_FILL
	progress=ProgressBar.new()
	progress.custom_minimum_size=Vector2(220,6)
	progress.show_percentage=false
	progress.visible=false
	line.add_child(progress)
	var log_toggle := Button.new()
	log_toggle.text="執行紀錄 ▾"
	log_toggle.toggle_mode=true
	log_toggle.toggled.connect(func(shown: bool): output.visible=shown)
	line.add_child(log_toggle)
	output=RichTextLabel.new()
	output.custom_minimum_size.y=105
	output.scroll_following=true
	main.add_child(output)
	output.hide()
	file_dialog=FileDialog.new()
	file_dialog.access=FileDialog.ACCESS_FILESYSTEM
	file_dialog.use_native_dialog=true
	file_dialog.mode_overrides_title=false
	file_dialog.current_dir=root.path_join("strategies")
	file_dialog.file_mode=FileDialog.FILE_MODE_OPEN_FILE
	file_dialog.file_selected.connect(func(path): _checkpoint(); _request(file_action,{"file":path}))
	add_child(file_dialog)
	notice=AcceptDialog.new()
	notice.title="策略研究工作台"
	add_child(notice)
	robustness_dialog=ConfirmationDialog.new()
	robustness_dialog.title="策略強健性檢測"
	robustness_dialog.ok_button_text="開始檢測"
	add_child(robustness_dialog)
	var robustness_form := VBoxContainer.new()
	robustness_dialog.add_child(robustness_form)
	for field in [
		["repetitions","每種行情情境次數",1,100,20],
		["noise_ticks","OHLC 雜訊上限（跳）",1,10,1],
		["block_bars","拔靴區塊長度（棒）",2,500,20],
		["seed","隨機種子",0,4294967295,20261122],
	]:
		var row := HBoxContainer.new()
		robustness_form.add_child(row)
		var label := _label(row,field[1],14,MUTED)
		label.size_flags_horizontal=Control.SIZE_EXPAND_FILL
		var input := SpinBox.new()
		input.min_value=field[2]
		input.max_value=field[3]
		input.value=field[4]
		input.custom_minimum_size.x=155
		row.add_child(input)
		robustness_fields[field[0]]=input
	robustness_dialog.confirmed.connect(_start_robustness)
	confirm=ConfirmationDialog.new()
	confirm.title="尚未儲存的修改"
	confirm.confirmed.connect(func(): if confirm_callback.is_valid(): confirm_callback.call())
	add_child(confirm)
	_build_version_page()
	pages.tab_changed.connect(func(page: int): _update_workspace_tabs(page,page_buttons))
	_update_workspace_tabs(EDIT_PAGE,page_buttons)

func _menu(parent: Control, caption: String, items: Array) -> MenuButton:
	var menu := MenuButton.new()
	menu.text=caption
	for state in ["normal","hover","pressed","focus","disabled"]:
		menu.add_theme_stylebox_override(state,theme.get_stylebox(state,"Button"))
	parent.add_child(menu)
	var popup := menu.get_popup()
	for item in items: popup.add_item(item[0])
	popup.index_pressed.connect(func(index: int):
		var callback: Callable=items[index][1]
		callback.call())
	return menu

func _select_workspace_page(page: int, selectors: Array[Button]) -> void:
	pages.current_tab=page
	_update_workspace_tabs(page,selectors)

func _update_workspace_tabs(page: int, selectors: Array[Button]) -> void:
	for index in range(selectors.size()):
		selectors[index].set_pressed_no_signal(index==page)
	if page==OPTIMIZE_PAGE:
		workspace_tools.text="AI 優化 ▾"
	elif page==VERSION_PAGE:
		workspace_tools.text="版本控制 ▾"
	else:
		workspace_tools.text="工具 ▾"

func _show_code_search() -> void:
	pages.current_tab=EDIT_PAGE
	code_search.get_parent().show()
	code_search.grab_focus()

func _build_version_page() -> void:
	var layout := VBoxContainer.new()
	layout.name="版本控制"
	layout.add_theme_constant_override("separation",8)
	pages.add_child(layout)
	var heading := _row(layout)
	version_status=_label(heading,"請從左側開啟策略",15,MUTED)
	version_status.size_flags_horizontal=SIZE_EXPAND_FILL
	version_status.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS
	_button(heading,"重新整理",func(): _version_request("list"))
	var split := HSplitContainer.new()
	split.size_flags_vertical=SIZE_EXPAND_FILL
	layout.add_child(split)
	version_list=ItemList.new()
	version_list.custom_minimum_size=Vector2(245,145)
	version_list.size_flags_vertical=Control.SIZE_EXPAND_FILL
	version_list.item_selected.connect(func(_index): _version_request("read"))
	split.add_child(version_list)
	var preview := TabContainer.new()
	preview.size_flags_horizontal=SIZE_EXPAND_FILL
	split.add_child(preview)
	version_source=_code_edit()
	version_source.name="版本 PL 原始碼"
	version_source.editable=false
	version_source.gutters_draw_line_numbers=true
	preview.add_child(version_source)
	version_parameters=_code_edit()
	version_parameters.name="版本回測設定"
	version_parameters.editable=false
	preview.add_child(version_parameters)
	var local := _row(layout)
	version_message=LineEdit.new()
	version_message.placeholder_text="輸入版本說明"
	version_message.max_length=200
	version_message.size_flags_horizontal=Control.SIZE_EXPAND_FILL
	local.add_child(version_message)
	_button(local,"提交版本",func(): _version_request("commit"),true)
	_button(local,"還原為草稿",func(): _version_request("draft"))
	_button(local,"套用到目前分頁",_confirm_version_apply)
	var remote := _row(layout)
	version_remote=LineEdit.new()
	version_remote.placeholder_text="https://github.com/擁有者/儲存庫"
	version_remote.size_flags_horizontal=SIZE_EXPAND_FILL
	remote.add_child(version_remote)
	version_branch=LineEdit.new()
	version_branch.placeholder_text="GitHub 分支"
	version_branch.custom_minimum_size.x=150
	remote.add_child(version_branch)
	_button(remote,"從 GitHub 擷取",func(): _version_request("fetch"))
	_button(remote,"推送到 GitHub",_confirm_version_push)
	version_confirm=ConfirmationDialog.new()
	version_confirm.title="確認推送策略版本"
	version_confirm.ok_button_text="推送"
	version_confirm.confirmed.connect(func(): _version_request("push",true))
	add_child(version_confirm)
	pages.tab_changed.connect(func(page):
		if page==VERSION_PAGE: _invalidate_version_history())

func _invalidate_version_history() -> void:
	version_list.clear()
	version_source.text=""
	version_parameters.text=""
	version_status.text="請從左側開啟策略" if active<0 else "正在讀取策略版本…"
	version_pending_refresh=true

func _confirm_version_push() -> void:
	var branch := version_branch.text.strip_edges()
	if branch.is_empty(): branch=version_branch.placeholder_text
	version_confirm.dialog_text="將策略原始碼、參數及相依函式的版本歷史推送至：\n%s\n分支：%s\n請確認儲存庫的存取權限。" % [version_remote.text.strip_edges(),branch]
	version_confirm.popup_centered(Vector2i(550,160))

func _confirm_version_apply() -> void:
	if version_list.get_selected_items().is_empty():
		version_status.text="請選擇要套用的版本"
		return
	confirm.title="套用策略版本"
	confirm.dialog_text="以選取的版本替換目前分頁的草稿內容，並保留此分頁的版本歷史。"
	confirm_callback=func(): _version_request("apply",true)
	confirm.popup_centered(Vector2i(550,145))

func _version_request(operation: String, confirmed := false) -> void:
	if busy or optimizer.running: return
	if active<0 or documents[active].kind!="strategy":
		version_status.text="請從左側開啟策略"
		return
	if operation in ["commit","apply"] and not _capture():
		version_status.text=status.text
		return
	var key: String=documents[active].get("managed_id") if documents[active].get("managed_id") is String else documents[active].id
	if key!=version_document_key:
		version_document_key=key
		version_branch.clear()
	var args := {"operation":operation,"document":documents[active],"message":version_message.text,
		"remote":version_remote.text,"branch":version_branch.text,"confirmed":confirmed}
	if operation in ["read","draft","apply"]:
		var selected := version_list.get_selected_items()
		if selected.is_empty():
			version_status.text="請選擇要還原的版本"
			return
		args.version=version_list.get_item_metadata(selected[0])
	if operation in ["commit","apply"]: _save_session()
	if operation=="list": version_pending_refresh=false
	_request("strategy_version",args)

func _code_edit() -> CodeEdit:
	var e := CodeEdit.new()
	var font := SystemFont.new()
	font.font_names=PackedStringArray(["Cascadia Code","Consolas","Microsoft JhengHei UI"])
	e.add_theme_font_override("font",font)
	e.add_theme_font_size_override("font_size",17)
	e.scroll_past_end_of_file=true
	return e

func _source_changed() -> void:
	if loading: return
	_dirty()
	_color_code()

func _color_code() -> void:
	var h := CodeHighlighter.new()
	h.number_color=Color("edbb7e")
	h.symbol_color=Color("9bb8df")
	h.function_color=Color("80a9ff")
	h.add_color_region("{","}",Color("aabed9"))
	h.add_color_region("//","",Color("aabed9"),true)
	h.add_color_region("\"","\"",Color("d5b681"))
	var keywords := "inputs input variables variable vars var if then begin end else for to downto true false and or not buy sell sellshort buytocover next bar at market stop limit this close contracts contract total open high low volume marketposition currentcontracts currentbar totaltrades"
	var regex := RegEx.new()
	regex.compile("[A-Za-z_][A-Za-z_0-9]*")
	for match in regex.search_all(editor.text):
		var word: String=match.get_string()
		if word.to_lower() in keywords.split(" "): h.add_keyword_color(word,TEAL)
	editor.syntax_highlighter=h

func new_strategy() -> void:
	if busy or optimizer.running: return
	_checkpoint()
	_request("new",{"number":documents.size()+1})

func _catalog_list() -> void:
	library_list.clear()
	for entry in catalog:
		if library_kind.selected==0 and entry.kind!="strategy": continue
		if library_kind.selected==1 and entry.kind!="function": continue
		if not search.text.is_empty() and not entry.name.to_lower().contains(search.text.to_lower()): continue
		var display_name: String=entry.name.trim_prefix("REA_")
		var i := library_list.add_item(("ƒ  " if entry.kind=="function" else "◇  ")+display_name)
		library_list.set_item_metadata(i,entry)
		library_list.set_item_tooltip(i,display_name+"\n"+entry.source)

func _dirty() -> void:
	if loading or active<0: return
	documents[active].dirty=true
	doc_tabs.set_tab_title(active,documents[active].name.trim_prefix("REA_")+" ●")
	source_info.text="尚未儲存 · 修改後須重新回測才會更新結果"
	if documents[active].get("result") is Dictionary: run_info.text="PL或設定已修改，圖表與數值為前次回測結果；請重新執行。"

func _capture() -> bool:
	if active<0: return false
	var d: Dictionary=documents[active]
	var p: Dictionary=d.task.get("parameters",{}).duplicate(true)
	for key in ["strategy_name","from","to"]: p[key]=fields[key].text.strip_edges()
	for key in ["from","to"]: p[key]=DT.canonical_datetime(fields[key].text)
	for key in ["warmup_bars","contracts","initial_capital","commission_per_side","slippage_ticks","point_value","max_position_contracts","max_entry_batches"]:
		var text: String=fields[key].text.strip_edges()
		if text.is_empty() and key in ["max_position_contracts","max_entry_batches"]: p[key]=null; continue
		if not text.is_valid_float(): _error("設定欄位須為數字："+key); return false
		if key in ["warmup_bars","contracts","max_position_contracts","max_entry_batches","slippage_ticks"]:
			if not text.is_valid_int(): _error("設定欄位須為整數："+key); return false
			p[key]=int(text)
		else: p[key]=float(text)
	var parsed = JSON.parse_string(inputs.text)
	if not parsed is Dictionary: _error("策略輸入參數須為JSON物件"); return false
	p.inputs=parsed
	p.pyramiding=["disabled","any_signal","different_signal"][pyramid.selected]
	d.task.parameters=p
	if d.task.get("report_settings") is Dictionary:
		for key in ["report_start","report_end"]:
			if d.task.report_settings.get(key) is String: d.task.report_settings[key]=DT.canonical_datetime(d.task.report_settings[key])
	d.task.database=fields.database.text.strip_edges()
	if unit.selected>0:
		if not fields.period.text.is_valid_int(): _error("K棒週期須為整數"); return false
		var market: Dictionary=d.task.get("market",{}).duplicate(true) if d.task.get("market") is Dictionary else {}
		market.merge({"symbol":fields.symbol.text.strip_edges(),"unit":["seconds","minutes","days"][unit.selected-1],"period":int(fields.period.text)},true)
		d.task.market=market
	else: d.task.erase("market")
	d.name=p.strategy_name
	d.source=editor.text
	return true

func _checkpoint() -> void:
	if active>=0:
		documents[active].source=editor.text
		_capture()
	_save_session()

func _show_document() -> void:
	if active<0: return
	loading=true
	var d: Dictionary=documents[active]
	editor.text=d.source
	var p: Dictionary=d.task.parameters
	for key in fields:
		var value = p.get(key,"")
		if key=="database": value=d.task.get("database","")
		if key=="symbol": value=d.task.get("market",{}).get("symbol","") if d.task.get("market") is Dictionary else ""
		if key=="period": value=d.task.get("market",{}).get("period",10) if d.task.get("market") is Dictionary else 10
		if key in ["warmup_bars","contracts","max_position_contracts","max_entry_batches","period","slippage_ticks"] and value!=null and str(value)!="": value=int(value)
		if key in ["from","to"]: value=DT.date_time(value)
		fields[key].text="" if value==null else str(value)
	unit.select(["seconds","minutes","days"].find(d.task.market.unit)+1 if d.task.get("market") is Dictionary else 0)
	pyramid.select(maxi(0,["disabled","any_signal","different_signal"].find(p.get("pyramiding","disabled"))))
	inputs.text=JSON.stringify(p.get("inputs",{}),"  ")
	advanced.text=JSON.stringify(DT.presentation(d.task),"  ")
	source_info.text=("函式 · 無法單獨回測" if d.kind=="function" else "策略")+" · "+str(d.origin if d.get("origin")!=null else "未命名草稿")
	_color_code()
	loading=false
	_clear_result()
	if d.get("result") is Dictionary: _request("view",{"receipt":d.result,"page":0})
	if pages.current_tab==VERSION_PAGE: _invalidate_version_history()

func _add_document(d: Dictionary) -> void:
	var keep_version_page := pages.current_tab==VERSION_PAGE and action=="open"
	for i in range(documents.size()):
		if documents[i].id==d.id: _activate(i); return
	loading=true
	documents.append(d)
	doc_tabs.add_tab(d.name.trim_prefix("REA_")+(" ●" if d.dirty else ""))
	active=documents.size()-1
	doc_tabs.current_tab=active
	loading=false
	_show_document()
	_save_session()
	if not keep_version_page:
		pages.current_tab=EDIT_PAGE
		editor.grab_focus()

func _activate(index: int) -> void:
	if loading or busy or optimizer.running or index<0 or index>=documents.size(): return
	if active>=0 and not _capture():
		loading=true; doc_tabs.current_tab=active; loading=false
		return
	active=index
	loading=true; doc_tabs.current_tab=index; loading=false
	_show_document()
	_save_session()

func _close_document(index: int) -> void:
	if busy or optimizer.running: return
	if active>=0 and not _capture(): return
	if documents[index].dirty:
		confirm.title="尚未儲存的修改"
		confirm.dialog_text="確定關閉「%s」並放棄未儲存的修改？" % documents[index].name
		confirm_callback=func(): _remove_document(index)
		confirm.popup_centered(Vector2i(600,160))
	else: _remove_document(index)

func _remove_document(index: int) -> void:
	loading=true
	documents.remove_at(index)
	doc_tabs.remove_tab(index)
	active=mini(index,documents.size()-1)
	if active>=0: doc_tabs.current_tab=active
	loading=false
	if active>=0: _show_document()
	else: _request("new",{"number":1})
	_save_session()

func _apply_advanced() -> void:
	if busy or optimizer.running or active<0: return
	var task = JSON.parse_string(advanced.text)
	if not task is Dictionary or not task.get("parameters") is Dictionary: _error("完整任務須為含parameters的JSON物件"); return
	for key in ["from","to"]:
		if task.parameters.get(key) is String: task.parameters[key]=DT.canonical_datetime(task.parameters[key])
	documents[active].source=editor.text
	documents[active].task=task
	documents[active].dirty=true
	_show_document()
	_dirty()

func _document_action(op: String) -> void:
	if busy or optimizer.running or not _capture(): return
	_save_session()
	_request(op,{"document":documents[active]})

func _run() -> void:
	if busy or optimizer.running or not _capture(): return
	if documents[active].kind!="strategy": _error("函式不能單獨回測，請開啟策略"); return
	_save_session()
	_clear_result()
	pages.current_tab=RESULT_PAGE
	_request("run",{"document":documents[active],"excel":excel.button_pressed,"destination":destination.text.strip_edges()})

func start_controlled_run() -> bool:
	if busy or optimizer.running or active<0 or documents[active].kind!="strategy": return false
	if not _capture(): return false
	_save_session()
	_request("run_controlled",{"document":documents[active]})
	return busy and action=="run_controlled"

func stop_controlled_run() -> void:
	if busy and action=="run_controlled": bridge.stop_controlled_run()

func _start_optimization() -> void:
	if busy or optimizer.running or not _capture(): return
	_save_session()
	optimizer.start(documents[active])

func select_market(db: String, instrument: Dictionary, source_kind: int) -> bool:
	if busy or optimizer.running or active<0 or not _capture(): return false
	var symbol: String=instrument.symbol
	var task: Dictionary=documents[active].task
	var previous = task.get("market")
	if not previous is Dictionary or previous.get("symbol")!=symbol or task.get("database")!=db:
		task.erase("market")
		task.parameters.tick_dataset_id=null
	task.parameters.price_scale=instrument.price_scale
	task.parameters.minimum_move=instrument.minimum_move
	task.parameters.tick_size=float(instrument.minimum_move)/float(instrument.price_scale)
	fields.database.text=db
	fields.symbol.text=symbol
	fields.point_value.text=str(instrument.point_value)
	unit.select(source_kind+1)
	fields.period.text=["30","1","1"][source_kind]
	_dirty()
	_checkpoint()
	advanced.text=JSON.stringify(DT.presentation(task),"  ")
	pages.current_tab=BACKTEST_PAGE
	return true

func _request(op: String,data: Dictionary = {}) -> void:
	if busy or bridge==null: return
	if optimizer.running and op not in ["optimize_baseline","optimize_candidate"]: return
	data=data.duplicate(true)
	data.action=op
	data.root=root
	if not bridge.start(JSON.stringify(data)): return
	action=op
	busy=true
	elapsed=0
	_set_busy(true)
	status.text={"init":"讀取策略庫…","open":"開啟策略…","check":"檢查相容語法…","save":"儲存策略副本…","run":"執行回測與報表流程…","run_controlled":"策略正在執行歷史回測…","view":"讀取本次報表…"}.get(op,"處理中…")
	if smoke: print("WORKBENCH_REQUEST: ",op)

func _version_completed(data: Dictionary) -> void:
	if data.has("versions"):
		version_branch.placeholder_text=data.branch
		version_status.text="%s｜%d 個本機版本" % [documents[active].name,data.versions.size()]
		version_list.clear()
		for version in data.versions:
			var index := version_list.add_item("%s  %s  %s" % [version.version.left(8),DT.date_time(version.date),version.message])
			version_list.set_item_metadata(index,version.version)
			version_list.set_item_tooltip(index,"%s\n%s" % [version.version,version.message])
		version_source.text=""
		version_parameters.text=""
		if not data.versions.is_empty():
			version_list.select(0)
			_version_request("read")
	elif data.get("schema")=="strategy-version-v1":
		version_source.text=data.source
		version_parameters.text=JSON.stringify(DT.presentation({"parameters":data.parameters,"market":data.market}),"  ")
	elif data.has("source"):
		_checkpoint()
		_add_document(data)
		_save_session()
		status.text="已開啟版本草稿"
	elif data.has("applied_document"):
		documents[active]=data.applied_document
		doc_tabs.set_tab_title(active,documents[active].name.trim_prefix("REA_"))
		_show_document()
		_save_session()
		status.text="版本已套用至目前草稿"
	elif data.get("action")=="push":
		version_status.text="已推送 %s 至 %s" % [data.version.left(8),data.branch]
	else:
		version_message.clear()
		output.append_text("\n策略版本：%s" % data.version)
		version_pending_refresh=true

func _set_busy(value: bool) -> void:
	value=value or optimizer.running
	for b in buttons:
		if is_instance_valid(b): b.disabled=value
	trade_replay_button.disabled=value or not last_view.get("receipt",{}).get("artifacts",{}).has("bars")
	report_menu.disabled=value or last_view.get("receipt",{}).get("status","")!="completed"
	workspace_tools.disabled=value
	progress.visible=value
	editor.editable=not value
	inputs.editable=not value
	advanced.editable=not value
	for f in fields.values(): f.editable=not value
	unit.disabled=value
	pyramid.disabled=value
	excel.disabled=value
	destination.editable=not value
	library_list.mouse_filter=Control.MOUSE_FILTER_IGNORE if value else Control.MOUSE_FILTER_STOP
	library_list.focus_mode=Control.FOCUS_NONE if value else Control.FOCUS_ALL
	doc_tabs.mouse_filter=Control.MOUSE_FILTER_IGNORE if value else Control.MOUSE_FILTER_STOP
	doc_tabs.focus_mode=Control.FOCUS_NONE if value else Control.FOCUS_ALL
	search.editable=not value
	version_list.mouse_filter=Control.MOUSE_FILTER_IGNORE if value else Control.MOUSE_FILTER_STOP
	version_list.focus_mode=Control.FOCUS_NONE if value else Control.FOCUS_ALL
	optimizer.set_workbench_busy(value)

func _process(delta: float) -> void:
	if not busy:
		if version_pending_refresh and pages.current_tab==VERSION_PAGE and not optimizer.running:
			version_pending_refresh=false
			_version_request("list")
		session_timer+=delta
		if session_timer>15 and active>=0:
			session_timer=0
			documents[active].source=editor.text
			_save_session()
		return
	elapsed+=delta
	progress.value=fmod(elapsed*17,100)
	var response: String=bridge.poll()
	if response.is_empty(): return
	var message = JSON.parse_string(response)
	if not message is Dictionary: return
	if message.get("type")=="log": output.append_text("\n"+message.text); return
	busy=false
	_set_busy(false)
	if not message.get("ok",false):
		if action=="strategy_version":
			version_status.text=message.get("error","版本作業失敗")
			status.text="版本作業未完成"
			operation_finished.emit(action,null,version_status.text)
			return
		if action in ["optimize_baseline","optimize_candidate"]:
			var reason: String=message.get("error","優化回測失敗")
			output.append_text("\n"+reason)
			status.text="優化回測未完成"
			optimizer.failed(action,reason)
			return
		if smoke and smoke_stage==4:
			_smoke_finish(true)
			return
		_error(message.get("error","工作失敗"))
		operation_finished.emit(action,null,message.get("error","工作失敗"))
		if smoke: get_tree().quit(1)
		return
	status.text="完成 · %.1f 秒" % elapsed
	output.append_text("\n"+Time.get_time_string_from_system()+"  "+action+" 完成")
	var finished := action
	_completed(finished,message.data)
	operation_finished.emit(finished,message.data,"")

func _completed(op: String,data: Variant) -> void:
	if smoke: print("WORKBENCH_COMPLETE: ",op, " ",str(data.get("receipt",{}).get("error","")) if data is Dictionary else "")
	if op=="init":
		catalog=data.catalog
		_catalog_list()
		if not smoke and _restore_session(): return
		_add_document(data.document)
	elif op in ["open","new","import","optimization_draft"]:
		_checkpoint()
		_add_document(data)
	elif op in ["optimize_baseline","optimize_candidate"]: optimizer.completed(op,data)
	elif op=="load_task":
		documents[active].task=data
		documents[active].dirty=true
		_show_document()
		pages.current_tab=BACKTEST_PAGE
	elif op=="save":
		documents[active]=data
		doc_tabs.set_tab_title(active,data.name.trim_prefix("REA_"))
		var entry := {"id":data.managed_id,"name":data.name,"kind":data.kind,"source":root.path_join("data/strategy-library/sources/"+data.managed_id+".pl"),"task":root.path_join("data/strategy-library/tasks/"+data.managed_id+".json"),"managed":true}
		var found := false
		for i in range(catalog.size()):
			if catalog[i].id==entry.id: catalog[i]=entry; found=true
		if not found: catalog.append(entry)
		_catalog_list()
		_save_session()
		source_info.text="策略副本已儲存，原始檔案保留"
	elif op=="check":
		status.text="語法檢查完成"
		output.append_text("\n"+data.message)
		notice.title="語法檢查"
		if DisplayServer.get_name()!="headless" and is_visible_in_tree():
			notice.dialog_text=data.message
			notice.popup_centered(Vector2i(520,155))
	elif op=="strategy_diff":
		status.text="已產生左右程式碼比對"
		output.append_text("\n"+data.report_path)
		if DisplayServer.get_name()!="headless": OS.shell_open(data.report_path)
	elif op=="strategy_export_pl":
		status.text="已匯出 PowerLanguage（CP950）"
		output.append_text("\n"+data.path)
		if DisplayServer.get_name()!="headless": OS.shell_open(str(data.path).get_base_dir())
	elif op=="strategy_logic":
		status.text="邏輯檢查完成 · %d 項發現" % data.finding_count
		output.append_text("\n%d 個流程節點｜%d 項邏輯發現" % [data.statement_count,data.finding_count])
		for finding in data.findings:
			output.append_text("\n"+finding.message)
		output.append_text("\n"+data.report_path+"\nExcalidraw  "+data.excalidraw_path)
		if DisplayServer.get_name()!="headless" and OS.shell_open(data.report_path)!=OK:
			output.append_text("\n無法開啟瀏覽器，可從上方路徑開啟流程圖。")
	elif op=="strategy_version": _version_completed(data)
	elif op=="robustness":
		robustness_report_path=data.report_path
		var lines := PackedStringArray()
		for method in [["ohlc_noise","OHLC 雜訊"],["moving_block_bootstrap","區塊拔靴"],["execution_stress","成交壓力"]]:
			var group: Dictionary=data.groups[method[0]]
			lines.append("%s｜完成 %d/%d，正淨利 %d，同棒違規 %d，淨利中位數 %s" % [
				method[1],group.completed,group.runs,group.positive_net,group.same_bar_violation_runs,
				"無" if group.net_closed.median==null else str(group.net_closed.median)])
		lines.append(data.report_path)
		output.append_text("\n"+ "\n".join(lines))
		status.text="強健性檢測完成，完整情境已存檔"
	elif op=="run_controlled" and data.receipt.status!="completed":
		status.text="策略回測已停止" if data.receipt.status=="stopped" else "策略回測失敗"
		output.append_text("\n"+str(data.receipt.get("error","回測未完成")))
	elif op in ["run","view","run_controlled"]:
		documents[active].result=data.receipt
		_save_session()
		_display_result(data)
	if smoke: call_deferred("_smoke_next",op,data)

func _clear_result() -> void:
	trade_replay_button.disabled=true
	report_menu.disabled=true
	for l in metrics.values(): l.text="—"
	chart.points=[]
	chart.queue_redraw()
	trades.clear()
	last_view={}
	trade_page=0
	trade_count=0
	page_info.text="交易明細尚未產生"
	run_info.text="尚無本次結果"

func _display_result(data: Dictionary) -> void:
	last_view=data
	var receipt: Dictionary=data.receipt
	if receipt.status!="completed":
		_clear_result()
		run_info.text="回測失敗，未產生成功報表"
		var reason := str(receipt.get("error","回測未完成"))
		if action=="run": _error(reason)
		else: output.append_text("\n"+reason)
		return
	var s: Dictionary=receipt.summary
	trade_replay_button.disabled=not receipt.get("artifacts",{}).has("bars")
	report_menu.disabled=receipt.get("status","")!="completed"
	var report_files: Dictionary=receipt.get("artifacts",{})
	var reports := report_menu.get_popup()
	reports.set_item_disabled(0,not report_files.has("html"))
	reports.set_item_disabled(1,not report_files.has("excel"))
	for key in metrics:
		metrics[key].text=("%d" % s[key]) if key=="trades" else (("%.2f%%" % s[key]) if key=="win_rate" else "%.2f" % s[key])
		metrics[key].add_theme_color_override("font_color",Color("f48c9c") if float(s[key])<0 else TEAL)
	chart.points=data.equity
	chart.full_count=int(data.equity_total)
	chart.queue_redraw()
	run_info.text="%s · %d 根K棒 · 曲線顯示 %d / %d 點；指標使用完整資料" % [receipt.run_id,int(data.get("bar_count",0)),data.equity.size(),int(data.equity_total)]
	var validation: Dictionary=receipt.get("trade_validation",{})
	if validation.get("status")=="passed":
		run_info.text+=" · 逐筆成交驗收通過"
	elif not validation.is_empty():
		run_info.text+=" · 逐筆成交驗收未通過（%d 筆），僅供對照" % int(validation.get("violation_count",0))
		if int(data.page)==0:
			for row: Dictionary in validation.get("violations",[]).slice(0,10):
				output.append_text("\n#%s 進場棒 %s（%s），出場棒 %s（%s）" % [int(row.trade_id),row.entry_bar,row.entry_label,row.exit_bar,row.exit_label])
			var audit_path: String=receipt.get("artifacts",{}).get("trade_validation","")
			if not audit_path.is_empty(): output.append_text("\n完整成交驗收明細："+audit_path)
	if data.get("open_trade") is Dictionary: run_info.text+=" · 期末仍有未平倉部位"
	var stats: Dictionary=receipt.get("report_statistics",{})
	if not stats.is_empty():
		run_info.text+="\n日均 %s 筆 · %d 個統計日 · %s" % ["%.2f" % stats.average_daily_trades if stats.get("average_daily_trades") != null else "—",int(stats.trading_days),stats.trading_day_basis]
	var coverage: Dictionary=receipt.get("market_coverage",{})
	if not coverage.is_empty():
		var check: Dictionary=coverage.get("last_check",{})
		var quality: String={"checked":"已檢查","stale":"檢查已過期","partial_range":"僅檢查部分期間","out_of_range":"檢查期間無重疊","source_mismatch":"檢查來源或版本不同","not_checked":"尚無檢查紀錄","unavailable":"無法讀取檢查紀錄"}.get(coverage.get("status"),"尚無檢查資訊")
		var totals: Dictionary=check.get("source_totals",{})
		if not totals.is_empty(): quality+=" · 檢查全期間缺漏 %d（%s）" % [int(totals.missing),totals.missing_unit]
		if not check.is_empty(): quality+=" · 上次檢查 %s%s" % [check.get("checked_at",""),"（已過期）" if check.get("stale",false) and coverage.status!="stale" else ""]
		run_info.text+="\n行情 %s · %s ～ %s（含暖機） · %s" % [coverage.get("source",""),DT.date_time(coverage.get("first_bar","")),DT.date_time(coverage.get("last_bar","")),quality]
		if int(data.page)==0:
			for gap: Dictionary in coverage.get("overlapping_gap_examples",[]):
				output.append_text("\n與回測重疊的缺口 %s ～ %s" % [gap.from,gap.to])
	trade_page=int(data.page)
	trade_count=int(data.trade_count)
	page_info.text="第 %d / %d 頁 · %d 筆" % [trade_page+1,maxi(1,ceili(trade_count/100.0)),trade_count]
	trades.clear()
	var tree_root := trades.create_item()
	for t in data.trades:
		var item := trades.create_item(tree_root)
		item.set_metadata(0,t)
		var values := [str(int(t.id)),("多" if t.side>0 else "空")+" / "+str(int(t.quantity)),DT.date_time(t.entry_datetime),DT.date_time(t.exit_datetime),str(t.entry_price),str(t.exit_price),"%.2f" % t.net,"%.2f" % t.cumulative,t.entry_label+" / "+t.exit_label]
		for i in range(values.size()): item.set_text(i,values[i]); item.set_tooltip_text(i,values[i])
		item.set_custom_color(6,TEAL if t.net>=0 else Color("f48c9c"))
	for warning in receipt.get("warnings",[]): output.append_text("\n"+str(warning))

func _open_robustness() -> void:
	robustness_dialog.popup_centered(Vector2i(485,215))

func _start_robustness() -> void:
	var receipt: Dictionary=last_view.get("receipt",{})
	var options := {}
	for key in robustness_fields:
		options[key]=int(robustness_fields[key].value)
	_request("robustness",{"run_id":receipt.get("run_id",""),"options":options})

func _open_trade_replay(selection := {}) -> void:
	var receipt: Dictionary=last_view.get("receipt",{})
	if receipt.get("status")!="completed" or not receipt.get("artifacts",{}).has("bars"): return
	if trade_replay==null:
		trade_replay=TradeReplay.new()
		trade_replay.visible=false
		trade_replay.theme=theme
		add_child(trade_replay)
	trade_replay.open_run(receipt,selection)

func _trade_page(delta: int) -> void:
	if busy or active<0 or not documents[active].get("result") is Dictionary: return
	var page := trade_page+delta
	if page<0 or page*100>=trade_count: return
	_request("view",{"receipt":documents[active].result,"page":page})

func _open_artifact(key: String) -> void:
	if active<0 or not documents[active].get("result") is Dictionary: _error("請先完成回測"); return
	var r: Dictionary=documents[active].result
	var path: String=str(r.get("result_path","")).get_base_dir() if key=="folder" else r.get("artifacts",{}).get(key,"")
	if path.is_empty() or (key!="folder" and not FileAccess.file_exists(path)): _error("本次尚無此報表，Excel須於回測前勾選產出"); return
	OS.shell_open(path)

func _find_code() -> void:
	if code_search.text.is_empty(): return
	var query := code_search.text
	editor.set_search_text(query)
	editor.set_search_flags(0)
	var hit := editor.search(query,0,editor.get_caret_line(),editor.get_caret_column()+1)
	if hit.x<0: hit=editor.search(query,0,0,0)
	if hit.x<0: status.text="找不到："+query; return
	editor.set_caret_line(hit.y)
	editor.set_caret_column(hit.x)
	editor.select(hit.y,hit.x,hit.y,hit.x+query.length())
	editor.grab_focus()

func _choose(op: String) -> void:
	if busy: return
	file_action=op
	file_dialog.filters=PackedStringArray(["*.pl,*.txt ; PowerLanguage 原始碼"] if op=="import" else ["*.json ; 回測任務 JSON"])
	file_dialog.title="匯入PL原始碼" if op=="import" else "讀入回測任務"
	file_dialog.popup_file_dialog()

func _error(text: String) -> void:
	status.text="需要處理 · "+text.left(85)
	output.append_text("\n"+text)
	if not smoke and is_visible_in_tree():
		notice.title="策略研究工作台"
		notice.dialog_text=text
		notice.popup_centered(Vector2i(750,220))

func _save_session() -> void:
	if smoke: return
	var session_docs := documents.duplicate(true)
	for d in session_docs: d.erase("view")
	var text := JSON.stringify({"documents":session_docs,"active":active})
	if text==last_session_text: return
	var recovery := FileAccess.open("user://recovery-%d.json" % OS.get_process_id(),FileAccess.WRITE)
	if recovery: recovery.store_string(text); recovery.close()
	var file := FileAccess.open("user://session.json",FileAccess.WRITE)
	if file: file.store_string(text); file.close(); last_session_text=text
	else: output.append_text("\n草稿工作階段儲存失敗，請使用儲存策略副本")

func _restore_session() -> bool:
	if not FileAccess.file_exists("user://session.json"): return false
	var data = JSON.parse_string(FileAccess.get_file_as_string("user://session.json"))
	if not data is Dictionary or not data.get("documents") is Array or data.documents.is_empty(): return false
	for d in data.documents:
		if not d is Dictionary or not d.get("task") is Dictionary or not d.get("source") is String: return false
	loading=true
	documents=data.documents
	for d in documents: doc_tabs.add_tab(d.name.trim_prefix("REA_")+(" ●" if d.dirty else ""))
	active=clampi(int(data.get("active",0)),0,documents.size()-1)
	doc_tabs.current_tab=active
	loading=false
	last_session_text=JSON.stringify({"documents":documents,"active":active})
	_show_document()
	return true

func _notification(what: int) -> void:
	if what==NOTIFICATION_WM_CLOSE_REQUEST and not embedded and prepare_close(): get_tree().quit()

func prepare_close() -> bool:
	if busy or optimizer.running:
		_error("回測或 AI 優化執行中，請等完成後關閉。")
		return false
	if active>=0 and not _capture(): return false
	_save_session()
	return true

func _unhandled_key_input(event: InputEvent) -> void:
	if is_visible_in_tree() and event is InputEventKey and event.pressed and not event.echo and not busy and not optimizer.running:
		if event.keycode==KEY_F5: _run(); get_viewport().set_input_as_handled()
		elif event.keycode==KEY_F7: _document_action("check"); get_viewport().set_input_as_handled()
		elif event.ctrl_pressed and event.keycode==KEY_S: _document_action("save"); get_viewport().set_input_as_handled()
		elif event.ctrl_pressed and event.keycode==KEY_F: _show_code_search(); get_viewport().set_input_as_handled()

func _smoke_next(op: String,data: Variant) -> void:
	if busy: return
	if smoke_stage==0 and op=="init":
		smoke_stage=1
		_request("import",{"file":root.path_join("strategies/pl-workbench-example.pl")})
	elif smoke_stage==1 and op=="import":
		excel.button_pressed=smoke_excel
		destination.text=smoke_dir
		_document_action("check")
	elif smoke_stage==1 and op=="check": _run()
	elif smoke_stage==1 and op=="run":
		if data.receipt.status!="completed" or data.trade_count<=0: _smoke_finish(false); return
		if smoke_excel and not FileAccess.file_exists(data.receipt.artifacts.get("excel","")): _smoke_finish(false); return
		if data.trade_count<=100:
			var net_sum := 0.0
			for t in data.trades: net_sum+=float(t.net)
			if not is_equal_approx(net_sum,float(data.receipt.summary.net_closed)): _smoke_finish(false); return
		smoke_receipts.append(data.receipt)
		first_net=float(data.receipt.summary.net_closed)
		smoke_stage=2
		excel.button_pressed=false
		editor.text=editor.text.replace("FastLength(10)","FastLength(15)").replace("SlowLength(30)","SlowLength(50)")
		_run()
	elif smoke_stage==2 and op=="run":
		if data.receipt.status!="completed" or is_equal_approx(first_net,float(data.receipt.summary.net_closed)): _smoke_finish(false); return
		smoke_receipts.append(data.receipt)
		smoke_stage=3
		editor.text="if UnknownFunction(Close)>0 then Buy next bar at market;"
		_run()
	elif smoke_stage==3 and op=="run":
		if data.receipt.status!="failed" or data.receipt.get("summary")!=null: _smoke_finish(false); return
		smoke_receipts.append(data.receipt)
		smoke_stage=4
		_document_action("check")

func _smoke_finish(passed: bool) -> void:
	if not smoke_dir.contains("godot-workbench-test"): get_tree().quit(1); return
	var file := FileAccess.open(smoke_dir.path_join("verification.json"),FileAccess.WRITE)
	if file: file.store_string(JSON.stringify({"passed":passed,"first_net":first_net,"excel_checked":smoke_excel,"receipts":smoke_receipts,"checks":["PL匯入","相容語法檢查","原碼回測","修改PL重新回測","摘要與交易檢視","逐筆損益對帳","不支援函式明確失敗"]},"  "))
	if passed: print("GODOT_WORKBENCH_SMOKE_OK")
	get_tree().quit(0 if passed else 1)
