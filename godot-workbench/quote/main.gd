extends Control

signal market_selected(database_path: String, instrument: Dictionary, source_kind: int)
signal operation_finished(operation: String, data: Variant, error: String)
var embedded := false

const INK = Color("f5f8ff")
const MUTED = Color("c0cee3")
const TEAL = Color("64f5d2")
const BLUE = Color("aec9ff")
const Coverage = preload("res://quote/coverage.gd")
const DT = preload("res://date_format.gd")
const ProductSymbols = preload("res://product_symbols.gd")
const DatePicker = preload("res://date_picker.gd")
var bridge: RefCounted
var root := ""
var database := ""
var inventory: Array = []
var current: Dictionary = {}
var current_sources: Array = []
var busy := false
var dirty := false
var loading := false
var action := ""
var fields: Dictionary = {}
var source_cards: Dictionary = {}
var sessions: VBoxContainer
var session_fields: Array = []
var product_list: VBoxContainer
var settings_tabs: TabContainer
var workspace_scroll: ScrollContainer
var settings_profile: Control
var save_feedback := false
var search: LineEdit
var headline: Label
var subtitle: Label
var total: Label
var status: Label
var log_text: RichTextLabel
var progress: ProgressBar
var coverage_chart: Control
var gap_text: RichTextLabel
var coverage_record: Label
var auto_fill: CheckBox
var from_date: Label
var to_date: Label
var kind: OptionButton
var role: OptionButton
var source_zone: LineEdit
var file_path: LineEdit
var file_dialog: FileDialog
var notice: AcceptDialog
var delete_dialog: ConfirmationDialog
var delete_code: LineEdit
var delete_data: CheckBox
var delete_summary: RichTextLabel
var delete_snapshot: Dictionary = {}
var format_dialog: ConfirmationDialog
var format_choice: OptionButton
var pending_import: Dictionary = {}
var confirm: ConfirmationDialog
var confirm_action: Callable
var buttons: Array[Button] = []
var selected_symbol := ""
var saving_symbol := ""
var range_symbol := ""
var last_coverage: Dictionary = {}
var tests: Array = []
var test_mode := false
var test_pending := false
var elapsed := 0.0
var test_stage := 0
var restore_test := false

func _ready() -> void:
	root = ProjectSettings.globalize_path("res://..").simplify_path()
	database = root.path_join("data/market.sqlite")
	var args := OS.get_cmdline_user_args()
	for arg in args:
		if arg.begins_with("--database="): database = arg.trim_prefix("--database=")
		if arg == "--smoke-test": test_mode = true
		if arg == "--smoke-restore": restore_test=true
	get_tree().auto_accept_quit = false
	_theme()
	_build()
	if not ClassDB.class_exists("QuoteManagerBridge"):
		_error("尚未載入 Rust 資料管理模組，請執行 quote-manager-godot.ps1 -Build")
		return
	bridge = ClassDB.instantiate("QuoteManagerBridge")
	_request("inventory")
	modulate.a = 0.0
	create_tween().tween_property(self,"modulate:a",1.0,0.4)

func _notification(what: int) -> void:
	if what == NOTIFICATION_WM_CLOSE_REQUEST and not embedded:
		if busy: _error("背景工作執行中，請等完成後再關閉。")
		elif dirty: _ask("商品有尚未儲存的修改，確定放棄並關閉？",func(): get_tree().quit())
		else: get_tree().quit()

func _style(color: Color, border: Color = Color("23334e"), radius: int = 12) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color = color
	box.border_color = border
	box.set_border_width_all(1)
	box.set_corner_radius_all(radius)
	box.content_margin_left = 18
	box.content_margin_right = 18
	box.content_margin_top = 14
	box.content_margin_bottom = 14
	return box

func _theme() -> void:
	var t := Theme.new()
	var font := SystemFont.new()
	font.font_names = PackedStringArray(["Microsoft JhengHei UI","Microsoft JhengHei"])
	t.default_font = font
	t.default_font_size = 16
	for cls in ["Label","Button","LineEdit","OptionButton","CheckBox","ItemList","RichTextLabel","TabContainer","TabBar"]:
		t.set_color("font_color",cls,INK)
		t.set_color("font_disabled_color",cls,MUTED)
	for cls in ["Button","OptionButton"]:
		for state in ["normal","hover","pressed","focus","disabled"]:
			var color := Color("15243b")
			if state == "hover": color = Color("233f5a")
			if state == "pressed": color = Color("1c5360")
			var b := _style(color,TEAL if state == "focus" else Color("2b405c"),8)
			b.content_margin_top=9; b.content_margin_bottom=9
			t.set_stylebox(state,cls,b)
	for state in ["normal","focus","read_only"]:
		var b := _style(Color("0c1526"),TEAL if state == "focus" else Color("253650"),7)
		b.content_margin_top=9; b.content_margin_bottom=9
		t.set_stylebox(state,"LineEdit",b)
	t.set_stylebox("panel","TabContainer",_style(Color("0e192b")))
	t.set_stylebox("tab_selected","TabContainer",_style(Color("243d56"),TEAL,7))
	t.set_stylebox("tab_unselected","TabContainer",_style(Color("101d31")))
	t.set_color("font_selected_color","TabContainer",TEAL)
	t.set_color("font_unselected_color","TabContainer",INK)
	t.set_color("font_placeholder_color","LineEdit",Color("becde3"))
	t.set_color("font_uneditable_color","LineEdit",Color("d5dfed"))
	t.set_stylebox("panel","ItemList",_style(Color("0c1627")))
	t.set_stylebox("selected","ItemList",_style(Color("1e4450"),TEAL,8))
	t.set_stylebox("selected_focus","ItemList",_style(Color("1e4450"),TEAL,8))
	t.set_constant("v_separation","ItemList",16)
	t.set_color("font_selected_color","ItemList",INK)
	t.set_stylebox("panel","PopupMenu",_style(Color("16253c")))
	t.set_stylebox("panel","AcceptDialog",_style(Color("16253c")))
	t.set_stylebox("background","ProgressBar",_style(Color("16243b")))
	t.set_stylebox("fill","ProgressBar",_style(TEAL))
	theme=t

func _label(parent: Node, text: String, font_size: int = 14, color: Color = INK) -> Label:
	var l := Label.new()
	l.text=text
	l.add_theme_font_size_override("font_size",maxi(14,font_size))
	l.add_theme_color_override("font_color",color)
	parent.add_child(l)
	return l

func _button(parent: Node,text: String,callback: Callable, primary: bool = false) -> Button:
	var b := Button.new()
	b.text=text
	b.size_flags_vertical=SIZE_SHRINK_CENTER
	if primary:
		b.add_theme_stylebox_override("normal",_style(Color("18534f"),Color("318779"),8))
		b.add_theme_color_override("font_color",TEAL)
	b.pressed.connect(callback)
	parent.add_child(b)
	buttons.append(b)
	return b

func _panel(parent: Node) -> VBoxContainer:
	var p := PanelContainer.new()
	p.add_theme_stylebox_override("panel",_style(Color("101d31")))
	p.size_flags_horizontal=SIZE_EXPAND_FILL
	parent.add_child(p)
	var v := VBoxContainer.new()
	v.add_theme_constant_override("separation",12)
	p.add_child(v)
	return v

func _row(parent: Node) -> HBoxContainer:
	var h := HBoxContainer.new()
	h.add_theme_constant_override("separation",12)
	parent.add_child(h)
	return h

func _field(parent: Node,caption: String,value: String = "") -> LineEdit:
	var v := VBoxContainer.new()
	v.size_flags_horizontal=SIZE_EXPAND_FILL
	parent.add_child(v)
	_label(v,caption,12,MUTED)
	var f := LineEdit.new()
	f.text=value
	f.size_flags_horizontal=SIZE_EXPAND_FILL
	v.add_child(f)
	return f

func _build() -> void:
	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	for side in ["left","right","top","bottom"]: margin.add_theme_constant_override("margin_"+side,24)
	add_child(margin)
	var main := VBoxContainer.new()
	main.add_theme_constant_override("separation",18)
	margin.add_child(main)
	var header := _row(main)
	var brand := VBoxContainer.new()
	brand.size_flags_horizontal=SIZE_EXPAND_FILL
	header.add_child(brand)
	_label(brand,"行情資料中心",30)
	_label(brand,"商品規格 · 交易時段 · 資料完整性",13,MUTED)
	total=_label(header,"讀取資料庫…",14,MUTED)
	_button(header,"重新整理",func(): _guard(func(): _request("inventory")))
	_button(header,"＋ 新增商品",_new_product,true)
	_button(header,"刪除商品…",func():
		if dirty: _error("請先儲存目前商品修改，再刪除商品"); return
		if selected_symbol.is_empty(): _error("請先選擇已儲存的商品"); return
		_request("delete_plan",{"symbol":selected_symbol}))
	var body := HBoxContainer.new()
	body.size_flags_vertical=SIZE_EXPAND_FILL
	body.add_theme_constant_override("separation",18)
	main.add_child(body)
	var nav := _panel(body)
	nav.custom_minimum_size.x=300
	nav.get_parent().size_flags_horizontal=SIZE_FILL
	_label(nav,"商品目錄",18)
	search=LineEdit.new()
	search.placeholder_text="搜尋代碼或商品名稱"
	search.text_changed.connect(func(_s): _list_products())
	nav.add_child(search)
	var product_scroll := ScrollContainer.new()
	product_scroll.size_flags_vertical=SIZE_EXPAND_FILL
	nav.add_child(product_scroll)
	product_list=VBoxContainer.new()
	product_list.size_flags_horizontal=SIZE_EXPAND_FILL
	product_list.add_theme_constant_override("separation",10)
	product_scroll.add_child(product_list)
	var scroll := ScrollContainer.new()
	workspace_scroll=scroll
	scroll.size_flags_horizontal=SIZE_EXPAND_FILL
	body.add_child(scroll)
	var workspace := VBoxContainer.new()
	workspace.size_flags_horizontal=SIZE_EXPAND_FILL
	workspace.add_theme_constant_override("separation",16)
	scroll.add_child(workspace)
	var heading := _row(workspace)
	var title_box := VBoxContainer.new()
	title_box.size_flags_horizontal=SIZE_EXPAND_FILL
	heading.add_child(title_box)
	headline=_label(title_box,"選擇商品",26)
	subtitle=_label(title_box,"資料來源依商品統一管理",13,MUTED)
	var cards := _row(workspace)
	for k in ["tick","minute","daily"]:
		var p := _panel(cards)
		var names := {"tick":"TICK / 逐筆成交","minute":"1 MIN / 1分K","daily":"DAY / 日K"}
		_label(p,names[k],12,TEAL if k=="tick" else BLUE)
		source_cards[k]={"count":_label(p,"—",28),"range":_label(p,"尚無來源",11,MUTED),"note":_label(p,"",11,MUTED)}
	var tabs := TabContainer.new()
	settings_tabs=tabs
	_button(heading,"編輯商品設定",_edit_product)
	_button(heading,"儲存商品設定",_save,true)
	if embedded:
		var backtest := OptionButton.new()
		backtest.add_item("帶入回測…")
		for label in ["秒 K（Tick）","1 分 K","日 K"]: backtest.add_item(label)
		backtest.item_selected.connect(func(index: int):
			backtest.select(0)
			if index==0 or busy: return
			if selected_symbol.is_empty() or dirty or not current.get("confirmed",false):
				_error("請先選擇商品並儲存確認過的規格。")
				return
			var source_key: String=["tick","minute","daily"][index-1]
			if not current_sources.any(func(source): return source.kind==source_key and int(source.row_count)>0):
				_error("所選商品沒有這類行情資料。")
				return
			market_selected.emit(database,current,index-1))
		heading.add_child(backtest)
	tabs.custom_minimum_size.y=400
	tabs.use_hidden_tabs_for_min_size=false
	workspace.add_child(tabs)
	var quality := VBoxContainer.new()
	quality.name="完整性與資料作業"
	quality.add_theme_constant_override("separation",12)
	tabs.add_child(quality)
	coverage_record=_label(quality,"尚無檢查紀錄",15,MUTED)
	coverage_record.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	_label(quality,"自動採用Tick／1分K／日K三來源的最早與最晚日期，同時檢查三來源，不限天數。",14,MUTED)
	var dates := _row(quality)
	var first_box := VBoxContainer.new()
	first_box.size_flags_horizontal=SIZE_EXPAND_FILL
	dates.add_child(first_box)
	_label(first_box,"資料最早日期（自動）",14,MUTED)
	from_date=_label(first_box,"—",18)
	var last_box := VBoxContainer.new()
	last_box.size_flags_horizontal=SIZE_EXPAND_FILL
	dates.add_child(last_box)
	_label(last_box,"資料最晚日期（自動）",14,MUTED)
	to_date=_label(last_box,"—",18)
	_button(dates,"檢查缺漏",func(): _operation("coverage"),true)
	_button(dates,"Tick 補建缺少K棒",func(): _operation("fill"))
	coverage_chart=Coverage.new()
	quality.add_child(coverage_chart)
	gap_text=RichTextLabel.new()
	gap_text.custom_minimum_size.y=100
	gap_text.text="尚未執行完整性檢查。涵蓋率依商品時段與手動休市日計算；Tick無成交僅列疑似缺漏。"
	quality.add_child(gap_text)
	var transfer_tab := VBoxContainer.new()
	transfer_tab.name="匯入與匯出"
	tabs.add_child(transfer_tab)
	var transfer := _panel(transfer_tab)
	_label(transfer,"資料匯入與匯出",17)
	var options := _row(transfer)
	kind=OptionButton.new()
	for s in ["TICK","1分K","日K"]: kind.add_item(s)
	options.add_child(kind)
	source_zone=_field(options,"來源時區","Asia/Taipei")
	role=OptionButton.new()
	for s in ["event","end","start","unspecified"]: role.add_item(s)
	options.add_child(role)
	kind.item_selected.connect(func(_index): _sync_source())
	var files := _row(transfer)
	file_path=_field(files,"CSV / TXT 檔案路徑")
	_button(files,"選擇檔案",func(): _choose_file(false))
	var operations := _row(transfer)
	_button(operations,"匯入資料",func(): _operation("import"),true)
	_button(operations,"匯出 CSV…",func(): _choose_file(true))
	_button(operations,"儲存來源標記",func(): _operation("annotate"))
	_label(transfer,"Tick按新檔起訖整段覆蓋；K棒相同時間後蓋前。補建只填缺少時間。",12,MUTED)
	var profile := VBoxContainer.new()
	settings_profile=profile
	profile.name="商品設定"
	profile.add_theme_constant_override("separation",12)
	tabs.add_child(profile)
	var identity := _row(profile)
	fields.symbol=_field(identity,"商品代碼")
	fields.name=_field(identity,"中文名稱")
	fields.english_name=_field(identity,"英文名稱（選填）")
	fields.timezone=_field(identity,"商品時區")
	var prices := _row(profile)
	fields.price_scale=_field(prices,"價格刻度 PriceScale")
	fields.minimum_move=_field(prices,"最小跳動 MinMove")
	fields.point_value=_field(prices,"每點價值")
	var calendar := _row(profile)
	fields.trading_weekdays=_field(calendar,"交易日星期（1=週一，逗號分隔）")
	fields.closed_dates=DatePicker.labeled(calendar,"休市開盤日","",false,true)
	_label(profile,"交易時段  /  星期以開盤日計，交易日位移0=當日、1=次交易日",13,TEAL)
	sessions=VBoxContainer.new()
	profile.add_child(sessions)
	_button(profile,"＋ 新增交易時段",func(): _add_session({"name":"時段","open":"09:00","close":"13:00","weekdays":[1,2,3,4,5],"trading_day_shift":0}); _dirty())
	auto_fill=CheckBox.new()
	auto_fill.text="Tick匯入成功後，自動補建缺少的1分K與日K（儲存後生效）"
	auto_fill.toggled.connect(func(_v): _dirty())
	profile.add_child(auto_fill)
	_button(profile,"儲存商品設定",_save,true)
	_label(profile,"時區標記不轉換原始時間。手動休市曆；衍生日K仍須核對Tick本身是否完整。",12,MUTED)
	for f in fields.values(): f.text_changed.connect(func(_v): _dirty())
	var footer := _panel(main)
	var status_row := _row(footer)
	status=_label(status_row,"準備就緒",13,TEAL)
	status.size_flags_horizontal=SIZE_EXPAND_FILL
	progress=ProgressBar.new()
	progress.custom_minimum_size=Vector2(240,6)
	progress.show_percentage=false
	progress.visible=false
	status_row.add_child(progress)
	log_text=RichTextLabel.new()
	log_text.custom_minimum_size.y=48
	log_text.scroll_following=true
	log_text.add_theme_color_override("default_color",MUTED)
	footer.add_child(log_text)
	file_dialog=FileDialog.new()
	file_dialog.access=FileDialog.ACCESS_FILESYSTEM
	file_dialog.use_native_dialog=true
	file_dialog.mode_overrides_title=false
	file_dialog.current_dir=OS.get_system_dir(OS.SYSTEM_DIR_DOWNLOADS)
	file_dialog.filters=PackedStringArray(["*.csv,*.txt ; 行情資料 CSV / TXT"])
	file_dialog.file_selected.connect(func(path): file_path.text=path; if file_dialog.file_mode==FileDialog.FILE_MODE_SAVE_FILE: _operation("export"))
	add_child(file_dialog)
	notice=AcceptDialog.new()
	notice.title="行情資料中心"
	add_child(notice)
	confirm=ConfirmationDialog.new()
	confirm.title="確認操作"
	confirm.confirmed.connect(func(): if confirm_action.is_valid(): confirm_action.call())
	add_child(confirm)
	delete_dialog=ConfirmationDialog.new()
	delete_dialog.title="刪除商品與行情"
	delete_dialog.ok_button_text="確認永久刪除"
	delete_dialog.cancel_button_text="取消"
	var deletion_box := VBoxContainer.new()
	delete_dialog.add_child(deletion_box)
	delete_summary=RichTextLabel.new()
	delete_summary.custom_minimum_size=Vector2(660,220)
	deletion_box.add_child(delete_summary)
	delete_data=CheckBox.new()
	delete_data.text="連同此商品全部行情刪除（含歷史與衍生資料集）"
	deletion_box.add_child(delete_data)
	delete_code=_field(deletion_box,"確認商品代碼（請完整輸入）")
	delete_code.text_changed.connect(func(_text): _update_delete_confirmation())
	delete_data.toggled.connect(func(_value): _update_delete_confirmation())
	delete_dialog.confirmed.connect(func(): _request("delete",{"symbol":delete_snapshot.symbol,"confirmation":delete_code.text.strip_edges(),"token":delete_snapshot.token,"delete_data":delete_data.button_pressed}))
	add_child(delete_dialog)
	format_dialog=ConfirmationDialog.new()
	format_dialog.title="檔案格式與目前選擇不同"
	format_dialog.ok_button_text="套用格式並檢視匯入確認"
	format_dialog.cancel_button_text="取消"
	var format_box := VBoxContainer.new()
	format_dialog.add_child(format_box)
	_label(format_box,"偵測到的欄位與目前選擇不同，請確認原檔的資料類型。",14)
	_label(format_box,"K棒欄位不能證明週期；其他分鐘K不能當1分K匯入。",14,MUTED)
	format_choice=OptionButton.new()
	format_box.add_child(format_choice)
	format_dialog.confirmed.connect(func():
		format_dialog.hide()
		kind.select(format_choice.get_selected_id())
		_sync_source()
		pending_import.kind=["tick","minute","daily"][kind.selected]
		pending_import.timezone=source_zone.text.strip_edges()
		pending_import.role=role.get_item_text(role.selected)
		_confirm_import())
	add_child(format_dialog)

func _confirm_import() -> void:
	var data := pending_import.duplicate(true)
	_ask("將匯入 %s 的 %s 資料。\nTick在新檔起訖內整段覆蓋；K棒相同時間後蓋前。\n來源時區：%s；時間標記：%s\n%s" % [data.symbol,{"tick":"Tick","minute":"1分K","daily":"日K"}[data.kind],data.timezone,data.role,data.file],func(): _request("import",data))
	if test_mode: call_deferred("_confirm_test_import")

func _confirm_test_import() -> void:
	if test_mode:
		confirm.hide()
		confirm.confirmed.emit()

func _confirm_test_format() -> void:
	if test_mode:
		format_dialog.hide()
		format_dialog.confirmed.emit()

func _update_delete_confirmation() -> void:
	if delete_snapshot.is_empty(): return
	delete_dialog.get_ok_button().disabled=delete_code.text.strip_edges()!=delete_snapshot.symbol or (not delete_snapshot.datasets.is_empty() and not delete_data.button_pressed)

func _dirty() -> void:
	if not loading:
		dirty=true
		subtitle.text="● 尚未儲存的修改"

func _ask(text: String, callback: Callable) -> void:
	confirm_action=callback
	confirm.dialog_text=text
	confirm.popup_centered(Vector2i(580,160))

func _guard(callback: Callable) -> void:
	if busy: return
	if dirty: _ask("商品有尚未儲存的修改，確定放棄修改？",callback)
	else: callback.call()

func _error(text: String) -> void:
	status.text="需要處理："+text.left(90)
	log_text.append_text("\n"+text)
	if is_visible_in_tree():
		notice.dialog_text=text
		notice.popup_centered(Vector2i(680,180))

func _list_products() -> void:
	for child in product_list.get_children(): product_list.remove_child(child); child.queue_free()
	for row in inventory:
		var i: Dictionary=row.instrument
		var english: String=i.get("english_name","")
		if not search.text.is_empty() and not (i.symbol+" "+i.name+" "+english).to_lower().contains(search.text.to_lower()): continue
		var button := Button.new()
		button.size_flags_horizontal=SIZE_EXPAND_FILL
		button.custom_minimum_size.y=78 if not english.is_empty() else 58
		button.tooltip_text=i.symbol+"\n"+i.name+("\n"+english if not english.is_empty() else "")
		if i.symbol==selected_symbol: button.add_theme_stylebox_override("normal",_style(Color("1e4450"),TEAL,8))
		button.pressed.connect(_product_clicked.bind(i.symbol))
		buttons.append(button)
		product_list.add_child(button)
		var margin := MarginContainer.new()
		margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
		margin.mouse_filter=Control.MOUSE_FILTER_IGNORE
		for side in ["left","right","top","bottom"]: margin.add_theme_constant_override("margin_"+side,12)
		button.add_child(margin)
		var lines := VBoxContainer.new()
		lines.mouse_filter=Control.MOUSE_FILTER_IGNORE
		margin.add_child(lines)
		var names := HBoxContainer.new()
		names.mouse_filter=Control.MOUSE_FILTER_IGNORE
		lines.add_child(names)
		_label(names,i.symbol,18,TEAL)
		var chinese := _label(names,i.name if i.name!=i.symbol else "未設定中文名稱",14,INK)
		chinese.size_flags_horizontal=SIZE_EXPAND_FILL
		chinese.horizontal_alignment=HORIZONTAL_ALIGNMENT_RIGHT
		chinese.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS
		if not english.is_empty():
			var en := _label(lines,english,14,MUTED)
			en.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS

func _product_clicked(symbol: String) -> void:
	_guard(func(): _select_symbol(symbol))
	_list_products()

func _edit_product() -> void:
	if busy: return
	if current.is_empty(): _error("請先選擇商品"); return
	settings_tabs.current_tab=2
	call_deferred("_focus_product_settings")

func _focus_product_settings() -> void:
	workspace_scroll.ensure_control_visible(settings_profile)
	fields.name.grab_focus()
	status.text="編輯 %s 的商品設定，修改後請按「儲存設定」" % current.symbol

func _select_symbol(symbol: String) -> void:
	for row in inventory:
		if row.instrument.symbol==symbol:
			selected_symbol=symbol
			_show_instrument(row.instrument)
			_show_sources(row.sources)
			if row.get("last_coverage") is Dictionary: _display_coverage(row.last_coverage)
			_list_products()
			return

func _show_instrument(i: Dictionary) -> void:
	loading=true
	current=i.duplicate(true)
	for key in fields:
		var value = i.get(key,"")
		if key=="minimum_move": value=int(value)
		if key=="trading_weekdays": value=value.map(func(n): return str(int(n)))
		if key=="closed_dates": value=value.map(func(d): return DT.date(d))
		fields[key].text=",".join(value) if value is Array else str(value)
	fields.symbol.editable=selected_symbol.is_empty()
	headline.text=i.symbol+"  /  "+i.name
	subtitle.text=("已確認商品規格" if i.get("confirmed",false) else "待確認商品規格，請檢查並儲存")+"  ·  "+i.timezone
	for child in sessions.get_children(): sessions.remove_child(child); child.queue_free()
	session_fields.clear()
	for s in i.sessions: _add_session(s)
	auto_fill.button_pressed=i.get("auto_fill",false)
	source_zone.text=i.timezone
	dirty=false
	loading=false
	coverage_chart.rows=[]
	coverage_chart.queue_redraw()
	gap_text.text="尚未檢查此商品。Tick無成交僅列疑似缺漏，不代表確認漏收。"
	last_coverage={}
	coverage_record.text="尚無檢查紀錄"
	coverage_record.add_theme_color_override("font_color",MUTED)

func _display_coverage(data: Dictionary) -> void:
	last_coverage=data
	coverage_chart.set_data(data.sources,float(data.timeline_from_ms),float(data.timeline_to_ms))
	gap_text.clear()
	for s in data.sources:
		if s.kind=="tick":
			gap_text.append_text("Tick缺口：%d 段，合計 %.3f 秒（同時段相鄰成交至少3分鐘）\n" % [s.gap_count,s.get("missing_seconds",0)])
			if not s.available or s.tick_count<2: gap_text.append_text("  無足夠Tick可判定相鄰成交缺口\n")
		else: gap_text.append_text("%s：%d ／預期 %d，%d 區段\n" % [{"minute":"1分K缺少時間","daily":"日K缺少交易日"}[s.kind],s.missing,s.expected,s.gap_count])
		if not s.get("warning","").is_empty(): gap_text.append_text("  "+s.warning+"\n")
		for gap in s.gaps:
			var detail: String="%.3f 秒" % gap.duration_seconds if s.kind=="tick" else "%d 棒" % int(gap.count)
			gap_text.append_text("  %s → %s（%s）\n" % [DT.date_time(gap.from),DT.date_time(gap.to),detail])
	var stale: bool=data.get("stale",false)
	coverage_record.text=("資料或商品設定已變更，請重新檢查。以下保留上次結果。\n" if stale else "最後一次檢查紀錄\n")+"檢查時間：%s（臺灣時間）　範圍：%s ～ %s" % [data.get("checked_at",""),DT.date(data.from),DT.date(data.to)]
	coverage_record.add_theme_color_override("font_color",Color("ffcf70") if stale else INK)

func _add_session(s: Dictionary) -> void:
	var h := _row(sessions)
	var row := {"container":h}
	for key in ["name","open","close","weekdays","trading_day_shift"]:
		var captions := {"name":"名稱","open":"開盤 HH:mm:ss","close":"收盤 HH:mm:ss","weekdays":"星期（逗號分隔）","trading_day_shift":"交易日位移"}
		var value = s[key]
		if key=="trading_day_shift": value=int(value)
		if key=="weekdays": value=value.map(func(n): return str(int(n)))
		if key in ["open","close"]: value=DT.time(value)
		row[key]=_field(h,captions[key],",".join(value) if value is Array else str(value))
		row[key].custom_minimum_size.x=75
		row[key].text_changed.connect(func(_v): _dirty())
	_button(h,"移除",func(): session_fields.erase(row); sessions.remove_child(h); h.queue_free(); _dirty())
	session_fields.append(row)

func _show_sources(sources: Array) -> void:
	current_sources=sources
	var earliest := ""
	var latest := ""
	for s in sources:
		if int(s.get("row_count",0))<=0: continue
		var a: String=str(s.get("first_datetime","")).left(10)
		var b: String=str(s.get("last_datetime","")).left(10)
		if a.length()!=10 or b.length()!=10: continue
		if earliest.is_empty() or a<earliest: earliest=a
		if b>latest: latest=b
	from_date.text=DT.date(earliest) if not earliest.is_empty() else "—"
	to_date.text=DT.date(latest) if not latest.is_empty() else "—"
	range_symbol=selected_symbol
	for k in source_cards:
		var card: Dictionary=source_cards[k]
		card.count.text="—"
		card.range.text="尚無來源"
		card.note.text="可由匯入建立"
		for s in sources:
			if s.kind!=k: continue
			card.count.text=_number(int(s.row_count))+" 筆"
			card.range.text=DT.date_time(s.get("first_datetime",""))+"\n"+DT.date_time(s.get("last_datetime",""))
			card.note.text="時間標記："+s.get("timestamp_role","未知")
			card.note.tooltip_text="時區："+s.get("timezone","未知")+"\nTick衍生：%d 筆；來源已變更：%d 筆" % [s.get("tick_derived_rows",0),s.get("tick_source_changed_rows",0)]
			if int(s.get("tick_source_changed_rows",0))>0: card.note.text+=" · 衍生資料待更新"
	_sync_source()

func _sync_source() -> void:
	var k: String=["tick","minute","daily"][kind.selected]
	source_zone.text=current.get("timezone","Asia/Taipei")
	role.select(0 if kind.selected==0 else 1)
	for s in current_sources:
		if s.kind!=k: continue
		source_zone.text=s.get("timezone","unspecified")
		for index in range(role.item_count):
			if role.get_item_text(index)==s.get("timestamp_role","unspecified"): role.select(index)

func _number(n: int) -> String:
	var s := str(n)
	var out := ""
	for i in range(s.length()):
		if i>0 and (s.length()-i)%3==0: out+=","
		out+=s[i]
	return out

func _new_product() -> void:
	_guard(func(): selected_symbol=""; _request("defaults",{"symbol":"NEW"}))

func _integers(text: String) -> Array:
	var a: Array=[]
	for part in text.split(",",false):
		if not part.strip_edges().is_valid_int(): return [-1]
		a.append(int(part.strip_edges()))
	return a

func _save() -> void:
	if current.is_empty(): return
	var i := current.duplicate(true)
	for key in ["symbol","name","english_name","timezone"]: i[key]=fields[key].text.strip_edges()
	var code: String=i.symbol.to_upper()
	if ProductSymbols.continuous(code)!=code:
		_error("請使用連續商品代碼。")
		return
	for key in ["price_scale","point_value"]:
		if not fields[key].text.is_valid_float(): _error("價格規格請輸入數字"); return
		i[key]=float(fields[key].text)
	if not fields.minimum_move.text.is_valid_int(): _error("最小跳動請輸入整數"); return
	i.minimum_move=int(fields.minimum_move.text)
	i.trading_weekdays=_integers(fields.trading_weekdays.text)
	i.closed_dates=[]
	for d in fields.closed_dates.text.split(",",false): i.closed_dates.append(DT.canonical_date(d))
	i.sessions=[]
	for row in session_fields:
		if not row.trading_day_shift.text.is_valid_int(): _error("交易日位移請輸入0或1"); return
		i.sessions.append({"name":row.name.text,"open":row.open.text,"close":row.close.text,"weekdays":_integers(row.weekdays.text),"trading_day_shift":int(row.trading_day_shift.text)})
	i.auto_fill=auto_fill.button_pressed
	saving_symbol=i.symbol.to_upper()
	if selected_symbol.is_empty():
		for row in inventory:
			if row.instrument.symbol==saving_symbol:
				_ask("商品代碼已存在，確定更新此商品的規格與時段？",func(): _request("save",{"instrument":i}))
				return
	_request("save",{"instrument":i})

func _operation(op: String) -> void:
	if current.is_empty() or selected_symbol.is_empty(): _error("請先新增並儲存商品，或選擇既有商品"); return
	if dirty: _error("請先儲存商品修改，再執行資料作業"); return
	var k: String=["tick","minute","daily"][kind.selected]
	var data := {"symbol":selected_symbol,"from":DT.canonical_date(from_date.text),"to":DT.canonical_date(to_date.text),"file":file_path.text.strip_edges(),"kind":k,"timezone":source_zone.text.strip_edges(),"role":role.get_item_text(role.selected)}
	if op in ["import","export"] and data.file.is_empty(): _error("請選擇檔案路徑"); return
	if op == "import":
		pending_import=data.duplicate(true)
		_request("inspect_import",{"file":data.file})
	elif op == "annotate":
		_ask("確認來源時間屬於 %s / %s？\n此操作只儲存中繼資料，不轉換原始時間。" % [data.timezone,data.role],func(): _request(op,data))
	elif op=="coverage": _request("coverage_all",data)
	elif op=="fill": _request("fill_all",data)
	else: _request(op,data)

func _choose_file(save_file: bool) -> void:
	if busy: return
	file_dialog.file_mode=FileDialog.FILE_MODE_SAVE_FILE if save_file else FileDialog.FILE_MODE_OPEN_FILE
	file_dialog.title="匯出行情 CSV" if save_file else "選擇匯入資料"
	file_dialog.filters=PackedStringArray(["*.csv ; CSV 檔案"] if save_file else ["*.csv,*.txt ; 行情資料 CSV / TXT","*.csv ; CSV 檔案","*.txt ; 文字檔案"])
	var path := file_path.text.strip_edges()
	if not path.is_empty() and DirAccess.dir_exists_absolute(path.get_base_dir()): file_dialog.current_dir=path.get_base_dir()
	if save_file: file_dialog.current_file=selected_symbol+"_"+["tick","minute","daily"][kind.selected]+".csv"
	file_dialog.popup_file_dialog()

func _request(op: String, data: Dictionary = {}) -> void:
	if busy or bridge==null: return
	if test_mode: print("SMOKE_REQUEST: ",op)
	data=data.duplicate(true)
	data.action=op
	data.database=database
	data.root=root
	if not bridge.start(JSON.stringify(data)): return
	action=op
	busy=true
	elapsed=0
	_set_busy(true)
	status.text={"inventory":"讀取商品與資料來源…","coverage":"正在掃描資料完整性…","fill":"正在從Tick補建缺少K棒…","import":"正在匯入資料…","export":"正在匯出資料…","save":"正在儲存商品…","defaults":"建立商品範本…","annotate":"儲存來源時間標記…"}.get(op,op)
	if op=="coverage_all": status.text="正在檢查三來源完整期間（不限天數）…"
	if op=="fill_all": status.text="正在按資料完整期間補建缺少K棒…"

func _set_busy(value: bool) -> void:
	progress.visible=value
	for b in buttons:
		if is_instance_valid(b): b.disabled=value
	product_list.mouse_filter=Control.MOUSE_FILTER_IGNORE if value else Control.MOUSE_FILTER_STOP
	product_list.focus_mode=Control.FOCUS_NONE if value else Control.FOCUS_ALL
	search.editable=not value
	for f in fields.values(): f.editable=not value
	if not selected_symbol.is_empty(): fields.symbol.editable=false
	for row in session_fields:
		for key in ["name","open","close","weekdays","trading_day_shift"]: row[key].editable=not value
	auto_fill.disabled=value
	kind.disabled=value
	role.disabled=value
	for f in [source_zone,file_path]: f.editable=not value

func _process(delta: float) -> void:
	if not busy: return
	elapsed+=delta
	progress.value=fmod(elapsed*16,100)
	var response: String=bridge.poll()
	if response.is_empty(): return
	busy=false
	_set_busy(false)
	var result = JSON.parse_string(response)
	if result==null or not result.get("ok",false):
		var message: String=result.get("error","回應格式錯誤") if result is Dictionary else "回應格式錯誤"
		if test_mode: push_error(message); get_tree().quit(1)
		else: _error(message)
		operation_finished.emit(action,null,message)
		return
	var finished := action
	_complete(finished,result.data)
	operation_finished.emit(finished,result.data,"")

func _complete(op: String,data: Variant) -> void:
	if test_mode: print("SMOKE_COMPLETE: ",op)
	status.text="完成 · %.1f 秒" % elapsed
	log_text.append_text("\n"+Time.get_time_string_from_system()+"  "+op+" 完成")
	if op=="inspect_import":
		var mismatch: bool=(pending_import.kind=="tick" and data.shape=="bar") or (pending_import.kind!="tick" and data.shape=="tick")
		if mismatch:
			format_choice.clear()
			if data.shape=="tick": format_choice.add_item("Tick（逐筆成交）",0)
			else:
				format_choice.add_item("1分K（僅限原檔實際為1分鐘週期）",1)
				format_choice.add_item("日K",2)
				format_choice.select(1 if data.suggested_kind=="daily" else 0)
			format_dialog.popup_centered(Vector2i(780,190))
			if test_mode and test_stage==4: call_deferred("_confirm_test_format")
		else: _confirm_import()
	elif op=="delete_plan":
		delete_snapshot=data
		delete_code.text=""
		delete_data.button_pressed=false
		delete_summary.text="商品：%s / %s\n資料集：%d 份（包含既有歷史與衍生資料）\n" % [data.symbol,data.name,data.datasets.size()]
		for item in data.datasets: delete_summary.append_text("  %s：%s 筆，%s → %s\n" % [{"trade":"Tick","minute":"1分K","daily":"日K"}.get(item.kind,item.kind),_number(int(item.rows)),DT.date_time(item.first),DT.date_time(item.last)])
		delete_summary.append_text("\n此操作無法從介面復原，請先匯出所需行情。原始匯入檔及既有報表保留。")
		delete_dialog.popup_centered(Vector2i(730,410))
		_update_delete_confirmation()
	elif op=="delete":
		selected_symbol=""
		current={}
		dirty=false
		log_text.append_text("\n"+data.note)
		_request("inventory")
	elif op=="inventory":
		inventory=data.filter(func(row): return ProductSymbols.continuous(row.instrument.symbol)==row.instrument.symbol)
		total.text="%d 個商品  ·  本機資料庫" % inventory.size()
		_list_products()
		if not selected_symbol.is_empty(): _select_symbol(selected_symbol)
		elif not inventory.is_empty():
			var first: String=inventory[0].instrument.symbol
			for row in inventory:
				if row.instrument.symbol=="TXF1": first="TXF1"
			_select_symbol(first)
		else:
			headline.text="尚無商品"
			subtitle.text="請新增商品並設定交易時段"
			_show_sources([])
		if save_feedback:
			save_feedback=false
			status.text="%s 商品設定已儲存並重新讀取" % selected_symbol
	elif op=="defaults":
		selected_symbol=""
		_show_instrument(data)
		_show_sources([])
		dirty=true
	elif op in ["coverage","coverage_all"]:
		_display_coverage(data)
		for row in inventory:
			if row.instrument.symbol==selected_symbol: row.last_coverage=data
	elif op in ["fill","fill_all"]:
		log_text.append_text("\n補建結果："+JSON.stringify(data.get("inserted",{}))+"\n"+data.get("warning",""))
		if data.get("status","")=="partial": _error(data.warning)
		_request("inventory")
	elif op in ["save","import","annotate"]:
		if op=="save": dirty=false; selected_symbol=saving_symbol; save_feedback=true
		if op=="import":
			log_text.append_text("\n匯入結果："+JSON.stringify(DT.presentation(data)))
			if data.get("auto_fill",{}) is Dictionary and data.auto_fill.get("status","")=="failed": _error("原始資料已匯入，但自動補建失敗："+data.auto_fill.get("error",""))
		_request("inventory")
	elif op=="export": log_text.append_text("\n已匯出 %d 筆至 %s" % [data.rows,data.path])
	if restore_test and op=="inventory":
		if not database.contains("godot-ui-test") or last_coverage.is_empty() or not last_coverage.get("stale",false) or coverage_chart.rows.size()!=3 or not coverage_record.text.contains("請重新檢查"):
			push_error("重開後未恢復最後檢查紀錄及過期提示"); get_tree().quit(1); return
		print("GODOT_COVERAGE_RESTORE_OK")
		get_tree().quit(0)
	if test_mode: call_deferred("_smoke_next","coverage" if op=="coverage_all" else op,data)

func _smoke_next(op: String,data: Variant) -> void:
	# 僅在明確指定測試旗標、獨立資料庫時寫入測試商品。
	if not database.contains("godot-ui-test"): push_error("測試必須使用獨立godot-ui-test資料庫"); get_tree().quit(1); return
	if busy: return
	if op=="inventory" and not test_pending:
		test_pending=true
		_request("defaults",{"symbol":"GODOT_TEST"})
	elif op=="defaults":
		fields.name.text="Godot 介面測試商品"
		from_date.text="2026-10-26"
		to_date.text="2026-10-26"
		session_fields[0].open.text="09:00"
		session_fields[0].close.text="09:03"
		auto_fill.button_pressed=true
		_save()
	elif op=="inventory" and test_pending:
		if current.get("symbol","")!="GODOT_TEST": push_error("商品儲存／重新讀取失敗"); get_tree().quit(1); return
		if test_stage==1:
			test_stage=2
			from_date.text="2020/01/01"
			to_date.text="2020/01/01"
			_operation("coverage")
		elif test_stage==0:
			test_stage=1
			file_path.text=database.get_base_dir().path_join("ticks.csv")
			_operation("import")
		elif test_stage==4:
			if last_coverage.is_empty() or not last_coverage.stale or coverage_chart.rows.size()!=3:
				push_error("匯入後未保留舊紀錄並提示重新檢查"); get_tree().quit(1); return
			test_stage=5
			_operation("coverage")
		elif test_stage==6:
			if current.name!="更新後中文名稱" or current.english_name!="Updated Product" or current.point_value!=250 or current.sessions[0].close!="09:04:00":
				push_error("既有商品設定更新／重新讀取失敗"); get_tree().quit(1); return
			_select_symbol("GODOT_TEST")
			var old_result: Dictionary=last_coverage.duplicate(true)
			old_result.from="2010-01-01"
			old_result.to="2011-01-01"
			_display_coverage(old_result)
			if from_date.text!="2026/10/26" or to_date.text!="2026/10/26":
				push_error("舊檢查紀錄覆蓋目前來源範圍"); get_tree().quit(1); return
			_select_symbol("GODOT_TEST")
			if last_coverage.is_empty() or not last_coverage.stale or not coverage_record.text.contains("請重新檢查"):
				push_error("切換後未恢復檢查紀錄"); get_tree().quit(1); return
			print("GODOT_UI_SMOKE_OK: 商品新增/設定更新重讀/Tick匯入/自動補建/三來源/缺漏/冪等補建/匯出/錯選Tick時切換1分K")
			get_tree().quit(0)
		else:
			test_stage=3
			file_path.text=database.get_base_dir().path_join("tick-export.csv")
			_operation("export")
	elif op=="coverage":
		if data.sources.size()!=3: get_tree().quit(1); return
		if data.from!="2026-10-26" or data.to!="2026-10-26": push_error("未自動採三來源最寬範圍"); get_tree().quit(1); return
		if test_stage==5:
			if data.sources[1].missing!=0 or kind.selected!=1: push_error("K棒格式切換／匯入失敗"); get_tree().quit(1); return
			test_stage=6
			_edit_product()
			if settings_tabs.current_tab!=2: push_error("商品設定入口失敗"); get_tree().quit(1); return
			fields.name.text="更新後中文名稱"
			fields.english_name.text="Updated Product"
			fields.point_value.text="250"
			session_fields[0].close.text="09:04:00"
			_save()
			return
		if data.sources[1].missing!=1 or data.sources[2].observed!=1: push_error("Tick補建或缺漏結果錯誤"); get_tree().quit(1); return
		_operation("fill")
	elif op=="export":
		if data.rows!=4: push_error("Tick合法重複未保留"); get_tree().quit(1); return
		test_stage=4
		kind.select(0)
		file_path.text=database.get_base_dir().path_join("bars.csv")
		_operation("import")
