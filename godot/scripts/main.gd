extends Control

const Chart = preload("res://scripts/chart.gd")
const SettingsDialog = preload("res://scripts/chart_settings.gd")
const INK = Color("e2eaf6")
const MUTED = Color("788ba5")
var engine: RefCounted
var raw_bars: Array = []
var chart: Control
var info_labels: Dictionary = {}
var source_label: Label
var title_label: Label
var status_label: Label
var view_label: Label
var period: OptionButton
var symbol: LineEdit
var import_dialog: FileDialog
var export_dialog: FileDialog
var message: AcceptDialog
var tool_buttons: Dictionary = {}
var display_checks: Dictionary = {}
var chart_style: OptionButton
var settings_dialog: ConfirmationDialog
var current_file := ""
var date_from := ""
var date_to := ""
var headline_price: Label
var headline_change: Label
var creating_chart := false
var data_source := "示範資料・非真實行情"
var export_kind := "csv"
var instrument := "TWSE 示範"
var active_tab := 0
var workspaces: Dictionary = {}
var tabs: TabBar
var tab_guard := false

func _ready() -> void:
	_build_theme()
	_build_ui()
	if not ClassDB.class_exists("ChartEngine"):
		message.dialog_text = "尚未載入 Rust 模組，請先執行 build.ps1，再重新啟動。"
		message.popup_centered()
		return
	engine = ClassDB.instantiate("ChartEngine")
	var preferences := ConfigFile.new()
	if preferences.load("user://appearance.cfg")==OK:
		chart.apply_palette(preferences.get_value("chart","palette",chart.palette()))
	var example := ProjectSettings.globalize_path("res://../TXF1-Day-Trade.txt")
	if FileAccess.file_exists(example): _read_csv(example)
	else: _load_demo(7)
	if "--smoke-test" in OS.get_cmdline_user_args():
		get_tree().create_timer(20.0).timeout.connect(func(): get_tree().quit(1))
		call_deferred("_smoke_test")
	if "--capture" in OS.get_cmdline_user_args(): call_deferred("_capture_preview")

func _build_theme() -> void:
	var t := Theme.new()
	var font := SystemFont.new()
	font.font_names = PackedStringArray(["Microsoft JhengHei UI","Microsoft JhengHei","Noto Sans CJK TC"])
	t.default_font = font
	t.default_font_size = 13
	for kind in ["Label","Button","OptionButton","CheckButton","MenuButton","LineEdit","TabBar"]:
		t.set_color("font_color",kind,INK)
		t.set_color("font_hover_color",kind,Color("69d6f4"))
		t.set_color("font_pressed_color",kind,Color("69d6f4"))
	for kind in ["Button","OptionButton","MenuButton"]:
		for state in ["normal","hover","pressed","focus"]:
			var box := StyleBoxFlat.new()
			box.bg_color = Color("203d52") if state=="pressed" else (Color("233044") if state=="hover" else Color("141e2e"))
			box.border_color = Color("5abedc") if state in ["pressed","focus"] else Color("25354b")
			box.set_border_width_all(1 if state!="normal" else 0)
			box.set_corner_radius_all(6)
			box.content_margin_left=10; box.content_margin_right=10
			box.content_margin_top=6; box.content_margin_bottom=6
			t.set_stylebox(state,kind,box)
	for state in ["normal","focus"]:
		var field := StyleBoxFlat.new()
		field.bg_color=Color("0c1422"); field.border_color=Color("2c3c54")
		field.set_border_width_all(1); field.set_corner_radius_all(3)
		field.content_margin_left=10; field.content_margin_right=10
		t.set_stylebox(state,"LineEdit",field)
	t.set_color("font_color","LineEdit",INK)
	t.set_color("font_selected_color","TabBar",Color("167cc4"))
	t.set_color("font_unselected_color","TabBar",MUTED)
	for kind in ["PopupMenu","CheckBox","TabContainer","SpinBox"]: t.set_color("font_color",kind,INK)
	for kind in ["PopupMenu","TabContainer","AcceptDialog"]:
		var panel := StyleBoxFlat.new(); panel.bg_color=Color("101a2a")
		panel.set_border_width_all(1); panel.border_color=Color("26394e"); panel.set_corner_radius_all(8)
		t.set_stylebox("panel",kind,panel)
	for state in ["tab_selected","tab_unselected","tab_hovered"]:
		var tab_style := StyleBoxFlat.new(); tab_style.bg_color=Color("1d3144") if state=="tab_selected" else Color("0e1725")
		tab_style.content_margin_left=16; tab_style.content_margin_right=16; tab_style.content_margin_top=10; tab_style.content_margin_bottom=10
		for kind in ["TabBar","TabContainer"]: t.set_stylebox(state,kind,tab_style)
	theme = t

func _label(text: String, color := INK, font_size := 13) -> Label:
	var l := Label.new(); l.text=text
	l.add_theme_color_override("font_color",color); l.add_theme_font_size_override("font_size",font_size)
	return l

func _panel(color: Color, minimum := Vector2.ZERO) -> PanelContainer:
	var p := PanelContainer.new()
	var box := StyleBoxFlat.new(); box.bg_color=color
	box.content_margin_left=12; box.content_margin_right=12; box.content_margin_top=6; box.content_margin_bottom=6
	p.add_theme_stylebox_override("panel",box); p.custom_minimum_size=minimum
	return p

func _row(parent: Node, color: Color, height: float) -> HBoxContainer:
	var p := _panel(color,Vector2(0,height)); parent.add_child(p)
	var row := HBoxContainer.new(); row.add_theme_constant_override("separation",6); p.add_child(row)
	return row

func _spacer(row: Node) -> void:
	var space := Control.new(); space.size_flags_horizontal=Control.SIZE_EXPAND_FILL; row.add_child(space)

func _separator(row: Node) -> void:
	row.add_child(VSeparator.new())

func _button(row: Node, text: String, action: Callable, tip := "") -> Button:
	var b := Button.new(); b.text=text; b.tooltip_text=tip; b.focus_mode=Control.FOCUS_NONE
	b.pressed.connect(action); row.add_child(b); return b

func _build_ui() -> void:
	var root := VBoxContainer.new(); root.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	root.add_theme_constant_override("separation",1); add_child(root)
	var heading := _row(root,Color("0c1321"),64)
	heading.add_child(_label("◈",Color("66deef"),31))
	heading.add_child(_label("RE / TERMINAL",INK,22))
	heading.add_child(_label("  期貨研究工作站",MUTED,12))
	_spacer(heading)
	source_label=_label("本機歷史資料",Color("66deef"),12); heading.add_child(source_label)
	_separator(heading)
	_button(heading,"＋ 新增圖表",func(): _open_settings(0,true),"設定商品與資料檔，建立新的圖表分頁")
	_button(heading,"圖表設定",func(): _open_settings(1))
	var menus := _row(root,Color("101928"),35)
	for entry in [["檔案",["匯入 CSV…","匯出 CSV…","匯出圖表 PNG…","載入示範資料"]],["檢視",["重設視野","顯示全部資料","切換十字線"]],["繪圖",["趨勢線","水平線","矩形","復原上一筆","清除繪圖"]],["說明",["操作說明"]]]:
		var menu := MenuButton.new(); menu.text=entry[0]; menus.add_child(menu)
		for item in entry[1]: menu.get_popup().add_item(item)
		menu.get_popup().id_pressed.connect(_menu.bind(entry[0]))
	_spacer(menus); menus.add_child(_label("◌  凱基期貨 API · 尚未連線",Color("d9b77d"),11))
	var toolbar := _row(root,Color("101928"),51)
	symbol=LineEdit.new(); symbol.text="TXF1"; symbol.custom_minimum_size=Vector2(145,34)
	symbol.tooltip_text="商品顯示名稱"; symbol.text_submitted.connect(_rename_symbol); toolbar.add_child(symbol)
	period=OptionButton.new(); period.custom_minimum_size=Vector2(85,34)
	for name in ["1 日","1 週","1 月"]: period.add_item(name)
	period.item_selected.connect(_change_period); toolbar.add_child(period)
	toolbar.add_child(_label("蠟燭 K 線",MUTED,12))
	_button(toolbar,"◑ 配色",func(): _open_settings(2))
	_separator(toolbar)
	for entry in [["cursor","↖ 游標"],["pan","✥ 平移"],["line","╱ 趨勢線"],["horizontal","― 水平線"],["rectangle","□ 矩形"]]:
		var key: String=entry[0]
		var b := _button(toolbar,entry[1],_set_tool.bind(key)); b.toggle_mode=true; tool_buttons[key]=b
	tool_buttons.cursor.button_pressed=true
	_spacer(toolbar)
	_button(toolbar,"匯入行情",_import_csv)
	_button(toolbar,"量能概況",func(): _open_settings(4),"保留頁籤，尚未實作")
	var body := HSplitContainer.new(); body.size_flags_vertical=Control.SIZE_EXPAND_FILL
	body.split_offset=238; root.add_child(body)
	var side := _panel(Color("101928"),Vector2(238,0)); body.add_child(side)
	var info := VBoxContainer.new(); info.add_theme_constant_override("separation",12); side.add_child(info)
	info.add_child(_label("INSTRUMENT / 商品",Color("66deef"),11))
	info.add_child(_label("TXF1",INK,28))
	info.add_child(_label("台指期連續月  ·  本機日線檔",MUTED,11))
	headline_price=_label("—",INK,33); info.add_child(headline_price)
	headline_change=_label("—",MUTED,13); info.add_child(headline_change)
	info.add_child(HSeparator.new())
	info.add_child(_label("BAR INSPECTOR / K 棒資料",Color("66deef"),11))
	for entry in [["date","日期"],["open","開盤價"],["high","最高價"],["low","最低價"],["close","收盤價"],["change","棒內漲跌"],["volume","成交量"]]:
		var row := HBoxContainer.new(); info.add_child(row)
		row.add_child(_label(entry[1],MUTED,12)); _spacer(row)
		var value := _label("—",INK,12); row.add_child(value); info_labels[entry[0]]=value
	info.add_child(HSeparator.new())
	var check := CheckBox.new(); check.text="十字線"; check.button_pressed=true
	check.toggled.connect(func(value): chart.crosshair=value; chart.queue_redraw()); info.add_child(check); display_checks.crosshair=check
	_button(info,"◑ 自訂 K 線顏色",func(): _open_settings(2))
	_button(info,"◎ 復原自適應",func(): chart.restore_auto_scale())
	var flexible := Control.new(); flexible.size_flags_vertical=Control.SIZE_EXPAND_FILL; info.add_child(flexible)
	info.add_child(_label("資料依原始檔呈現\n未接收即時行情",MUTED,11))
	var chart_column := VBoxContainer.new(); chart_column.add_theme_constant_override("separation",0)
	chart_column.size_flags_horizontal=Control.SIZE_EXPAND_FILL; body.add_child(chart_column)
	var chart_header := _row(chart_column,Color("0e1725"),44)
	title_label=_label("TXF1  /  1 日",INK,16); chart_header.add_child(title_label)
	_spacer(chart_header); chart_header.add_child(_label("CANDLESTICK  /  歷史資料",MUTED,11))
	chart=Chart.new(); chart.size_flags_vertical=Control.SIZE_EXPAND_FILL; chart.size_flags_horizontal=Control.SIZE_EXPAND_FILL
	chart.apply_palette({"up":"ff5d73","down":"2ed5ba","wick":"b7c6d9","background":"0b101b","grid":"1b2738"})
	chart_column.add_child(chart)
	chart.bar_hovered.connect(_show_bar); chart.viewport_changed.connect(func(text): view_label.text=text)
	chart.tool_finished.connect(func(): _set_tool("cursor"))
	tabs=TabBar.new(); tabs.custom_minimum_size.y=38
	for name in ["TXF1 · 日線研究","研究圖表 02","研究圖表 03"]: tabs.add_tab(name)
	tabs.tab_changed.connect(_switch_tab); chart_column.add_child(tabs)
	var footer := _row(root,Color("111b2b"),46)
	_button(footer,"↔ −",func(): chart.zoom(1.25),"水平縮小")
	_button(footer,"↔ ＋",func(): chart.zoom(0.8),"水平放大")
	_separator(footer)
	_button(footer,"↕ −",func(): chart.adjust_price_scale(1.2),"垂直縮小，顯示更大價格範圍")
	_button(footer,"↕ ＋",func(): chart.adjust_price_scale(0.8),"垂直放大")
	_button(footer,"↑",func(): chart.shift_price(0.1),"價格範圍上移")
	_button(footer,"↓",func(): chart.shift_price(-0.1),"價格範圍下移")
	_button(footer,"◎ 復原自適應",func(): chart.restore_auto_scale(),"只復原價格軸，保留水平視野")
	_separator(footer)
	_button(footer,"◀",func(): chart.move_bars(-chart.visible_count*0.3))
	_button(footer,"▶",func(): chart.move_bars(chart.visible_count*0.3))
	_button(footer,"全部",func(): chart.fit_all())
	_button(footer,"復原繪圖",_undo)
	_spacer(footer); view_label=_label("",MUTED,11); footer.add_child(view_label)
	var status := _row(root,Color("0b1220"),28)
	status_label=_label("就緒",MUTED,11); status.add_child(status_label); _spacer(status)
	status.add_child(_label("●  GODOT × RUST  /  本機研究模式",Color("5b8b9c"),10))
	import_dialog=FileDialog.new(); import_dialog.access=FileDialog.ACCESS_FILESYSTEM
	import_dialog.file_mode=FileDialog.FILE_MODE_OPEN_FILE; import_dialog.filters=PackedStringArray(["*.csv,*.txt ; 日線行情檔"])
	import_dialog.title="匯入日線資料"; import_dialog.file_selected.connect(_read_csv); add_child(import_dialog)
	export_dialog=FileDialog.new(); export_dialog.access=FileDialog.ACCESS_FILESYSTEM
	export_dialog.file_mode=FileDialog.FILE_MODE_SAVE_FILE; export_dialog.file_selected.connect(_export_file); add_child(export_dialog)
	message=AcceptDialog.new(); message.title="RE TERMINAL"; add_child(message)
	settings_dialog=SettingsDialog.new(); settings_dialog.theme=theme; add_child(settings_dialog)
	settings_dialog.configuration_applied.connect(_apply_settings)
func _set_tool(value: String) -> void:
	chart.set_tool(value)
	for key in tool_buttons: tool_buttons[key].button_pressed=key==value
	status_label.text="繪圖：點選起點與終點；右鍵取消" if value in ["line","rectangle"] else ("繪圖：點選價格建立水平線" if value=="horizontal" else "就緒  ·  滾輪縮放 / 拖曳平移")

func _rename_symbol(value: String) -> void:
	instrument=value.strip_edges()
	if instrument.is_empty(): instrument="未命名商品"
	_update_title()

func _update_title() -> void:
	title_label.text=instrument+"  /  "+period.get_item_text(period.selected)
	source_label.text=data_source
	if not chart.bars.is_empty():
		var latest: Dictionary=chart.bars.back()
		headline_price.text="%.0f" % latest.close
		var delta: float=latest.close-latest.open
		if chart.bars.size()>1: delta=latest.close-chart.bars[-2].close
		headline_change.text="%+.0f  ·  %s"%[delta,latest.date]
		headline_change.add_theme_color_override("font_color",chart.up_color if delta>=0 else chart.down_color)

func _open_settings(tab_index: int, create_new := false) -> void:
	creating_chart=create_new
	settings_dialog.open_config({"instrument":instrument,"path":current_file,"period":period.selected,"from":date_from,"to":date_to,"auto":chart.auto_scale,"palette":chart.palette()},tab_index)

func _valid_date(value: String) -> bool:
	if value.is_empty(): return true
	var pattern := RegEx.new(); pattern.compile("^[0-9]{4}-[0-9]{2}-[0-9]{2}$")
	if pattern.search(value)==null: return false
	var parts := value.split("-")
	var year := int(parts[0]); var month := int(parts[1]); var day := int(parts[2])
	if year<1 or month<1 or month>12 or day<1: return false
	var leap := year%4==0 and (year%100!=0 or year%400==0)
	var days := [31,29 if leap else 28,31,30,31,30,31,31,30,31,30,31]
	return day<=days[month-1]

func _apply_settings(config: Dictionary) -> void:
	if not _valid_date(config.from) or not _valid_date(config.to): _alert("日期請填有效的 YYYY-MM-DD"); return
	if not config.from.is_empty() and not config.to.is_empty() and config.from>config.to:
		_alert("開始日期不可晚於結束日期"); return
	var candidate: Array=raw_bars
	if config.path!=current_file:
		var file := FileAccess.open(config.path,FileAccess.READ)
		if file==null: _alert("無法讀取選擇的行情檔"); return
		var parsed: Dictionary=JSON.parse_string(engine.csv_json(file.get_as_text()))
		if not parsed.error.is_empty(): _alert(parsed.error); return
		candidate=parsed.bars
	var selected := candidate.filter(func(b): return (config.from.is_empty() or b.date>=config.from) and (config.to.is_empty() or b.date<=config.to))
	if selected.is_empty(): _alert("選擇的日期範圍沒有資料"); return
	var previous_auto: bool=chart.auto_scale
	var previous_mid: float=chart.fixed_mid
	var previous_spread: float=chart.fixed_spread
	if creating_chart:
		tabs.add_tab(config.instrument if not config.instrument.is_empty() else "新圖表")
		tabs.current_tab=tabs.tab_count-1
		creating_chart=false
	raw_bars=candidate; current_file=config.path; date_from=config.from; date_to=config.to
	instrument=config.instrument if not config.instrument.is_empty() else "TXF1"
	symbol.text=instrument; period.select(config.period)
	if not current_file.is_empty(): data_source="本機日線 · "+current_file.get_file()
	_change_period(config.period)
	chart.apply_palette(config.palette)
	if config.auto: chart.restore_auto_scale()
	elif not previous_auto:
		chart.auto_scale=false; chart.fixed_mid=previous_mid; chart.fixed_spread=previous_spread
	var preferences := ConfigFile.new(); preferences.set_value("chart","palette",config.palette)
	if preferences.save("user://appearance.cfg")!=OK: status_label.text="配色已套用，但無法儲存偏好設定"
	_update_title(); chart.queue_redraw()

func _load_demo(seed: int) -> void:
	if engine==null: return
	var payload: Dictionary=JSON.parse_string(engine.demo_json(seed))
	raw_bars=payload.bars
	current_file=""; date_from=""; date_to=""
	data_source="示範資料・非真實行情"
	chart.drawings.clear(); _change_period(period.selected)

func _change_period(index: int) -> void:
	if engine==null or raw_bars.is_empty(): return
	var selected := raw_bars.filter(func(b): return (date_from.is_empty() or b.date>=date_from) and (date_to.is_empty() or b.date<=date_to))
	var payload: Dictionary=JSON.parse_string(engine.aggregate_json(JSON.stringify(selected),index))
	if not payload.error.is_empty(): _alert(payload.error); return
	chart.drawings.clear(); chart.set_bars(payload.bars)
	_update_title()
	if not payload.bars.is_empty(): _show_bar(payload.bars.back())
	status_label.text="已彙整 %d 根 %s K 線；切換週期會清除目前繪圖"%[payload.bars.size(),period.get_item_text(index)]

func _show_bar(bar: Dictionary) -> void:
	for key in info_labels:
		if key=="change":
			var change: float=bar.close-bar.open
			info_labels[key].text="%+.2f"%change
			info_labels[key].add_theme_color_override("font_color",chart.up_color if change>=0 else chart.down_color)
		elif key=="date": info_labels[key].text=bar.date
		elif bar.get(key)==null: info_labels[key].text="—"
		else: info_labels[key].text=("%.0f" if key=="volume" else "%.2f")%bar[key]

func _switch_tab(index: int) -> void:
	if tab_guard or engine==null: return
	workspaces[active_tab]={"raw":raw_bars.duplicate(true),"period":period.selected,"instrument":instrument,"source":data_source,"drawings":chart.drawings.duplicate(true),"first":chart.first,"count":chart.visible_count,"auto":chart.auto_scale,"mid":chart.fixed_mid,"spread":chart.fixed_spread,"path":current_file,"from":date_from,"to":date_to}
	active_tab=index
	if workspaces.has(index):
		var saved: Dictionary=workspaces[index]
		raw_bars=saved.raw; instrument=saved.instrument; data_source=saved.source; period.select(saved.period)
		current_file=saved.path; date_from=saved.from; date_to=saved.to
		_change_period(saved.period); chart.drawings=saved.drawings; chart.first=saved.first; chart.visible_count=saved.count
		chart.auto_scale=saved.auto; chart.fixed_mid=saved.mid; chart.fixed_spread=saved.spread
	else:
		instrument="研究圖表 %02d"%(index+1); period.select(0); chart.style=0; _load_demo(7+index*19)
	symbol.text=instrument; _update_title(); chart.queue_redraw()

func _import_csv() -> void:
	import_dialog.popup_centered_ratio(0.65)

func _read_csv(path: String) -> void:
	if engine==null: _alert("Rust 模組尚未載入"); return
	var file := FileAccess.open(path,FileAccess.READ)
	if file==null: _alert("無法讀取檔案"); return
	var payload: Dictionary=JSON.parse_string(engine.csv_json(file.get_as_text()))
	if not payload.error.is_empty(): _alert(payload.error); return
	raw_bars=payload.bars; data_source="本機日線 · "+path.get_file(); current_file=path; date_from=""; date_to=""
	instrument="TXF1" if path.get_file().begins_with("TXF1") else path.get_file().get_basename()
	symbol.text=instrument; _change_period(period.selected)

func _ask_export(kind: String) -> void:
	export_kind=kind
	export_dialog.filters=PackedStringArray(["*.csv ; 行情 CSV"] if kind=="csv" else ["*.png ; 圖表 PNG"])
	export_dialog.current_file="recharts."+kind
	export_dialog.popup_centered_ratio(0.65)

func _export_file(path: String) -> void:
	if export_kind=="csv":
		var file := FileAccess.open(path,FileAccess.WRITE)
		if file==null: _alert("無法寫入檔案"); return
		file.store_csv_line(PackedStringArray(["date","open","high","low","close","volume"]))
		for bar in chart.bars:
			file.store_csv_line(PackedStringArray([bar.date,str(bar.open),str(bar.high),str(bar.low),str(bar.close),str(bar.volume)]))
	else:
		export_dialog.hide()
		await RenderingServer.frame_post_draw
		var image := get_viewport().get_texture().get_image()
		var result := image.save_png(path)
		if result!=OK: _alert("圖片儲存失敗：%d"%result); return
	status_label.text="已匯出 "+path.get_file()

func _undo() -> void:
	if not chart.drawings.is_empty(): chart.drawings.pop_back()
	chart.pending={}; chart.queue_redraw()

func _alert(text: String) -> void:
	message.dialog_text=text; message.popup_centered(Vector2i(560,190))

func _menu(id: int, category: String) -> void:
	match category:
		"檔案":
			match id:
				0: _import_csv()
				1: _ask_export("csv")
				2: _ask_export("png")
				3: _load_demo(7)
		"檢視":
			match id:
				0: chart.reset_view()
				1: chart.fit_all()
				_:
					var prop: String="crosshair"
					display_checks[prop].button_pressed = not chart.get(prop)
		"繪圖":
			match id:
				0: _set_tool("line")
				1: _set_tool("horizontal")
				2: _set_tool("rectangle")
				3: _undo()
				4: chart.drawings.clear(); chart.pending={}; chart.queue_redraw()
		"說明": _alert("滑鼠滾輪縮放、拖曳平移、雙擊重設。\n拖曳右側價格軸調整縱向縮放。\n趨勢線與矩形：先點起點，再點終點。\n快捷鍵：Ctrl+O 匯入、Ctrl+Z 復原、Home 重設、Esc 取消繪圖。\nCSV：date,open,high,low,close,volume；日期 YYYY-MM-DD。\n週線依 ISO 週、月線依日曆月彙整；週期切換清除繪圖。\n示範資料為 Rust 產生，不是真實行情；未提供券商下單。")

func _unhandled_key_input(event: InputEvent) -> void:
	if not event is InputEventKey or not event.pressed or event.echo: return
	if get_viewport().gui_get_focus_owner() is LineEdit: return
	if event.ctrl_pressed and event.keycode==KEY_O: _import_csv()
	elif event.ctrl_pressed and event.keycode==KEY_Z: _undo()
	elif event.keycode==KEY_ESCAPE: _set_tool("cursor")
	elif event.keycode==KEY_HOME: chart.reset_view()
	elif event.keycode==KEY_LEFT: chart.move_bars(-10)
	elif event.keycode==KEY_RIGHT: chart.move_bars(10)
	elif event.keycode in [KEY_PLUS,KEY_EQUAL]: chart.zoom(0.8)
	elif event.keycode==KEY_MINUS: chart.zoom(1.25)

func _capture_preview() -> void:
	await get_tree().process_frame
	await RenderingServer.frame_post_draw
	DirAccess.make_dir_recursive_absolute(ProjectSettings.globalize_path("res://../artifacts"))
	get_viewport().get_texture().get_image().save_png(ProjectSettings.globalize_path("res://../artifacts/main-window.png"))
	_open_settings(2)
	await get_tree().process_frame
	await RenderingServer.frame_post_draw
	get_viewport().get_texture().get_image().save_png(ProjectSettings.globalize_path("res://../artifacts/chart-settings.png"))
	get_tree().quit()

func _smoke_test() -> void:
	await get_tree().process_frame
	assert(raw_bars.size()>250,"Rust demo data missing")
	var daily_count: int = chart.bars.size()
	period.select(1); _change_period(1)
	assert(chart.bars.size()<daily_count and chart.bars.size()>40,"Weekly aggregation failed")
	period.select(2); _change_period(2)
	assert(chart.bars.size()<daily_count/4,"Monthly aggregation failed")
	period.select(0); _change_period(0)
	var previous: float=chart.visible_count
	chart.zoom(0.8); assert(chart.visible_count<previous,"Zoom failed")
	chart.move_bars(-10); chart.reset_view()
	await get_tree().process_frame
	var horizontal_first: float=chart.first
	var horizontal_count: float=chart.visible_count
	chart.adjust_price_scale(0.8)
	assert(not chart.auto_scale,"Manual price scale failed")
	var old_mid: float=chart.fixed_mid
	chart.shift_price(0.1); assert(chart.fixed_mid>old_mid,"Price axis movement failed")
	chart.restore_auto_scale()
	assert(chart.auto_scale and chart.first==horizontal_first and chart.visible_count==horizontal_count,"Auto scale must preserve horizontal view")
	var original_palette: Dictionary=chart.palette()
	var changed_palette: Dictionary=original_palette.duplicate()
	changed_palette.up="abcdef"; chart.apply_palette(changed_palette)
	assert(chart.up_color.to_html(false)=="abcdef","Candle color failed")
	chart.apply_palette(original_palette)
	assert(_valid_date("2024-02-29") and not _valid_date("2025-02-29") and not _valid_date("2026-13-01"),"Date validation failed")
	chart.drawings.append({"kind":"horizontal","a":{"date":raw_bars[10].date,"price":raw_bars[10].close},"b":{"date":raw_bars[10].date,"price":raw_bars[10].close}})
	_undo(); assert(chart.drawings.is_empty(),"Undo failed")
	_switch_tab(1); _switch_tab(0)
	assert(chart.bars.size()==daily_count,"Workspace restore failed")
	var imported: Dictionary=JSON.parse_string(engine.csv_json(FileAccess.get_file_as_string("res://../data/sample.csv")))
	assert(imported.error.is_empty() and imported.bars.size()==17,"CSV import failed")
	assert(settings_dialog.pages.get_tab_count()==5,"Chart configuration tabs missing")
	assert(not chart.show_ma and not chart.show_volume and not chart.show_roc,"First version must show candles only")
	print("RECHARTS_SMOKE_OK: file import, daily/weekly/monthly, axis scale/shift/auto restore, colors, date validation, drawing undo, workspaces, 5 settings tabs")
	get_tree().quit()
