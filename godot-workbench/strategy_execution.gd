extends "res://skin.gd"

signal reports_requested(profile: String)
const ForeignOrder = preload("res://foreign_order.gd")
const DatePicker = preload("res://date_picker.gd")
const CONFIG_PATH := "user://strategy-executions.cfg"
const PRODUCTS := {"TXF":"臺指期","MXF":"小型臺指期貨","TMF":"微型臺指期貨","CDF":"台積電期貨","225M":"小型日經 225 期貨"}
const LIVE_PRODUCTS := ["TXF","MXF","TMF"]

var workbench: Control
var search: LineEdit
var combinations: Tree
var strategy_picker: OptionButton
var product_picker: OptionButton
var period_minutes: SpinBox
var warmup_bars: SpinBox
var input_grid: GridContainer
var input_fields := {}
var strategy_message: Label
var save_button: Button
var delete_button: Button
var add_button: Button
var config_message: Label
var configurations: Array = []
var selected_id := ""
var changed := false
var loading := false
var profiles: OptionButton
var default_profile := ""
var month_info: Label
var split: HSplitContainer
var refresh_month: Timer
var switch: CheckButton
var phase: Label
var details: Label
var enable_checks: Label
var confirmation: ConfirmationDialog
var bridge: RefCounted
var available: Array = []
var pending := {}
var running := false
var stopping := false
var active_run := ""
var execution_states := {}
var execution_names := {}
var foreign_order: Control
var replay_button: Button
var replay_dialog: ConfirmationDialog
var replay_from: HBoxContainer
var replay_quarterly: CheckButton
var replay_to: HBoxContainer
var replay_proof := {}

func _ready() -> void:
	set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	var margins := MarginContainer.new()
	margins.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	for side in ["left","right","top","bottom"]: margins.add_theme_constant_override("margin_"+side,22)
	add_child(margins)
	var layout := VBoxContainer.new()
	layout.add_theme_constant_override("separation",14)
	margins.add_child(layout)
	var header := _row(layout)
	var title := _label(header,"策略交易管理",25,INK)
	title.size_flags_horizontal=SIZE_EXPAND_FILL
	_button(header,"海外期貨下單",_open_foreign_order)
	replay_button=_button(header,"225M 自動模擬",func():
		if running: stop()
		else: replay_dialog.popup_centered(Vector2i(460,155)))
	_button(header,"交易回報",func(): reports_requested.emit(_profile()))
	split=HSplitContainer.new()
	split.dragger_visibility=SplitContainer.DRAGGER_VISIBLE
	split.add_theme_constant_override("autohide",0)
	split.add_theme_constant_override("minimum_grab_thickness",24)
	split.add_theme_constant_override("separation",24)
	split.split_offset=55
	split.size_flags_vertical=SIZE_EXPAND_FILL
	layout.add_child(split)
	var left := _panel(split)
	left.get_parent().custom_minimum_size.x=470
	left.get_parent().size_flags_horizontal=SIZE_FILL
	var list_header := _row(left)
	var heading := _label(list_header,"執行搭配",18,INK)
	heading.size_flags_horizontal=SIZE_EXPAND_FILL
	add_button=_button(list_header,"新增搭配",_add_combination,true)
	search=LineEdit.new()
	search.placeholder_text="搜尋策略、商品或帳戶"
	left.add_child(search)
	search.text_changed.connect(func(_text): _render_list())
	combinations=Tree.new()
	combinations.columns=5
	combinations.hide_root=true
	combinations.select_mode=Tree.SELECT_ROW
	combinations.column_titles_visible=true
	for index in range(5):
		combinations.set_column_title(index,["狀態","策略","商品","週期","帳戶"][index])
		combinations.set_column_custom_minimum_width(index,[75,125,82,72,100][index])
	combinations.size_flags_vertical=SIZE_EXPAND_FILL
	combinations.item_selected.connect(_select_combination)
	left.add_child(combinations)
	var scroll := ScrollContainer.new()
	scroll.custom_minimum_size.x=440
	scroll.size_flags_horizontal=SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode=ScrollContainer.SCROLL_MODE_DISABLED
	split.add_child(scroll)
	var right := VBoxContainer.new()
	right.size_flags_horizontal=SIZE_EXPAND_FILL
	right.add_theme_constant_override("separation",12)
	scroll.add_child(right)
	_setup_split_handle.call_deferred()
	var strategy_setup := _panel(right)
	_label(strategy_setup,"策略",18,INK)
	strategy_picker=OptionButton.new()
	strategy_picker.fit_to_longest_item=false
	strategy_picker.size_flags_horizontal=SIZE_EXPAND_FILL
	strategy_setup.add_child(strategy_picker)
	strategy_picker.item_selected.connect(func(_index): _load_inputs({},-1); _edited())
	var warm := _row(strategy_setup)
	_label(warm,"暖機棒數",14,MUTED)
	warmup_bars=_integer(warm,0,1799,50)
	warmup_bars.value_changed.connect(func(_value): _edited())
	input_grid=GridContainer.new()
	input_grid.columns=2
	input_grid.add_theme_constant_override("h_separation",12)
	input_grid.add_theme_constant_override("v_separation",4)
	strategy_setup.add_child(input_grid)
	strategy_message=_label(strategy_setup,"",14,MUTED)
	strategy_message.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	var setup := _panel(right)
	_label(setup,"商品",18,INK)
	var market_row := _row(setup)
	product_picker=OptionButton.new()
	for code in PRODUCTS:
		product_picker.add_item("%s · %s" % [code,PRODUCTS[code]])
		product_picker.set_item_metadata(product_picker.item_count-1,code)
	product_picker.item_selected.connect(func(_index): _update_month(); _edited())
	market_row.add_child(product_picker)
	period_minutes=_integer(market_row,1,240,1)
	period_minutes.suffix="分 K"
	period_minutes.tooltip_text="以即時 1 分 K 合成指定週期，收棒後評估"
	period_minutes.value_changed.connect(func(_value): _edited())
	month_info=_label(setup,"",14,MUTED)
	month_info.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	var trade := _panel(right)
	var trade_row := _row(trade)
	_label(trade_row,"交易帳戶",14,MUTED)
	profiles=OptionButton.new()
	profiles.item_selected.connect(func(_index): _edited())
	trade_row.add_child(profiles)
	var actions := _row(right)
	save_button=_button(actions,"儲存搭配",_save_combination,true)
	delete_button=_button(actions,"移除搭配",_delete_combination)
	config_message=_label(right,"",14,MUTED)
	config_message.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	var controls := _panel(right)
	var control_row := _row(controls)
	switch=CheckButton.new()
	switch.text="啟用自動交易"
	switch.add_theme_font_size_override("font_size",19)
	switch.size_flags_horizontal=SIZE_EXPAND_FILL
	switch.toggled.connect(_toggle)
	control_row.add_child(switch)
	phase=_label(control_row,"已關閉",18,TEAL)
	details=_label(controls,"選擇策略、帳戶與契約後即可啟用。",15,MUTED)
	details.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	enable_checks=_label(controls,"",14,MUTED)
	enable_checks.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	_label(controls,"一次啟用一組正式交易。採單口市價 IOC，開始評估前須為空手。",14,MUTED)
	confirmation=ConfirmationDialog.new()
	confirmation.title="確認啟用正式交易"
	confirmation.ok_button_text="確認啟用"
	confirmation.cancel_button_text="取消"
	confirmation.confirmed.connect(_start)
	confirmation.canceled.connect(func(): pending={})
	add_child(confirmation)
	replay_dialog=ConfirmationDialog.new()
	replay_dialog.title="225M_Intraday 歷史行情自動模擬"
	replay_dialog.ok_button_text="開始模擬"
	add_child(replay_dialog)
	var replay_fields := _row(replay_dialog)
	replay_from=DatePicker.labeled(replay_fields,"起日","2025/05/27")
	replay_to=DatePicker.labeled(replay_fields,"迄日","2025/06/26")
	replay_quarterly=CheckButton.new()
	replay_quarterly.text="驗證連續行情接續"
	replay_quarterly.toggled.connect(func(on):
		if on:
			replay_from.text="2026/06/03"
			replay_to.text="2026/06/17")
	replay_dialog.add_child(replay_quarterly)
	replay_quarterly.button_pressed=true
	replay_dialog.confirmed.connect(_start_replay)
	if ClassDB.class_exists("LiveStrategyBridge"): bridge=ClassDB.instantiate("LiveStrategyBridge")
	refresh_month=Timer.new()
	refresh_month.wait_time=10.0
	refresh_month.timeout.connect(func():
		if is_visible_in_tree(): _update_month())
	add_child(refresh_month)
	refresh_month.start()
	workbench.operation_finished.connect(func(operation: String,_data: Variant,_error: String):
		if operation in ["init","save"] and not running: reload())
	var stored := ConfigFile.new()
	if stored.load(CONFIG_PATH)==OK:
		var data: Variant=stored.get_value("execution","rows",[])
		if data is Array:
			for row in data:
				if not row is Dictionary: continue
				for key in ["continuous_month","instrument","maturity","contract"]: row.erase(key)
				configurations.append(row)
	reload()
	if not configurations.is_empty(): _show_combination(configurations[0])
	else: _add_combination()

func _open_foreign_order() -> void:
	if foreign_order==null:
		foreign_order=ForeignOrder.new()
		add_child(foreign_order)
	foreign_order.open_for(_profile())

func _start_replay() -> void:
	if running or bridge==null: return
	replay_proof={}
	var from := DatePicker.canonical_date(replay_from.text)
	var to := DatePicker.canonical_date(replay_to.text)
	if not bridge.start_replay(JSON.stringify({"from":from,"to":to,
		"quarterly":replay_quarterly.button_pressed})): return
	running=true
	details.text="正在回放 225M 歷史行情與海外模擬委託…"
	_sync()

func _setup_split_handle() -> void:
	var handle := split.get_drag_area_control()
	if handle==null: return
	handle.mouse_default_cursor_shape=CURSOR_HSIZE
	handle.tooltip_text="拖曳調整執行搭配清單寬度"
	handle.draw.connect(func():
		var center := handle.size*0.5
		handle.draw_line(Vector2(center.x,0),Vector2(center.x,handle.size.y),Color("324762"),1.0)
		var color := TEAL if Rect2(Vector2.ZERO,handle.size).has_point(handle.get_local_mouse_position()) else Color("849bb7")
		handle.draw_line(center-Vector2(0,13),center+Vector2(0,13),color,3.0,true))
	handle.mouse_entered.connect(handle.queue_redraw)
	handle.mouse_exited.connect(handle.queue_redraw)
	handle.queue_redraw()

func _integer(parent: Container, low: int, high: int, value: int) -> SpinBox:
	var input := SpinBox.new()
	input.min_value=low
	input.max_value=high
	input.value=value
	parent.add_child(input)
	return input

func _selected(picker: OptionButton) -> String:
	if picker.selected<0: return ""
	return str(picker.get_item_metadata(picker.selected))

func _pick(picker: OptionButton, code: String) -> void:
	picker.select(0 if picker.item_count>0 else -1)
	for index in picker.item_count:
		if str(picker.get_item_metadata(index))==code:
			picker.select(index)
			return

func reload() -> void:
	if running: return
	var strategy := _strategy()
	var profile := _profile()
	if not changed and not selected_id.is_empty():
		profile=str(_stored().get("profile",""))
	elif not default_profile.is_empty() and profile==default_profile:
		profile=""
	available=workbench.catalog
	execution_names.clear()
	strategy_picker.clear()
	strategy_picker.add_item("選擇已儲存的策略")
	strategy_picker.set_item_metadata(0,"")
	for entry: Dictionary in available:
		if entry.kind!="strategy": continue
		var title: String=str(entry.name).trim_prefix("REA_")
		execution_names[entry.id]=title
		strategy_picker.add_item(title)
		strategy_picker.set_item_metadata(strategy_picker.item_count-1,entry.id)
	_pick(strategy_picker,strategy)
	var accounts := PackedStringArray()
	if ClassDB.class_exists("ApiProfileBridge"):
		var api: RefCounted=ClassDB.instantiate("ApiProfileBridge")
		var response: Variant=JSON.parse_string(api.request(workbench.root,"api_profile_list","{}"))
		if response is Dictionary and response.get("ok",false):
			for row in response.data.profiles:
				var setting: Dictionary=row.profile
				if setting.provider=="kgi_tradecom" and setting.purpose=="trade": accounts.append(setting.id)
	_set_profile_options(accounts,profile)
	_render_list()
	_sync()

func _set_profile_options(accounts: PackedStringArray, preferred: String) -> void:
	profiles.clear()
	default_profile=accounts[0] if accounts.size()==1 else ""
	if accounts.size()!=1:
		profiles.add_item("尚未設定交易帳戶" if accounts.is_empty() else "選擇交易帳戶")
		profiles.set_item_metadata(0,"")
	for account in accounts:
		profiles.add_item(account)
		profiles.set_item_metadata(profiles.item_count-1,account)
	_pick(profiles,preferred)

func _profile_needs_rebind() -> bool:
	var saved: String=_stored().get("profile","")
	return not saved.is_empty() and saved!=_profile()

func _load_inputs(saved: Dictionary, warmup: int) -> void:
	for child in input_grid.get_children():
		input_grid.remove_child(child)
		child.queue_free()
	input_fields.clear()
	strategy_message.text=""
	var values: Dictionary=saved
	var defaults: Variant=null
	if bridge!=null and not _strategy().is_empty():
		defaults=JSON.parse_string(bridge.strategy_settings(_strategy()))
	if defaults is Dictionary and defaults.get("ok",false):
		values=defaults.data.inputs.duplicate()
		values.merge(saved,true)
		warmup_bars.set_value_no_signal(int(defaults.data.warmup) if warmup<0 else warmup)
	else:
		warmup_bars.set_value_no_signal(50 if warmup<0 else warmup)
		if defaults is Dictionary:
			strategy_message.text=str(defaults.get("error","無法讀取策略參數。"))
	var keys := values.keys()
	keys.sort()
	for key in keys:
		_label(input_grid,str(key),14,MUTED)
		var field := LineEdit.new()
		field.text=str(values[key])
		field.size_flags_horizontal=SIZE_EXPAND_FILL
		input_grid.add_child(field)
		field.text_changed.connect(func(_text): _edited())
		input_fields[key]=field
	strategy_message.visible=not strategy_message.text.is_empty()

func _render_list() -> void:
	combinations.clear()
	var root := combinations.create_item()
	var filter := search.text.strip_edges().to_lower()
	for row: Dictionary in configurations:
		var name: String=execution_names.get(row.strategy,row.strategy)
		var code: String=row.product
		var label: String=PRODUCTS.get(code,code)
		if not filter.is_empty() and not (name+" "+code+" "+label+" "+str(row.profile)).to_lower().contains(filter): continue
		var item := combinations.create_item(root)
		item.set_metadata(0,row.id)
		for i in range(5): item.set_selectable(i,true)
		item.set_tooltip_text(1,name)
		item.set_text(0,execution_states.get(row.id,"已關閉"))
		item.set_text(1,name)
		item.set_text(2,code)
		item.set_tooltip_text(2,label)
		item.set_text(3,"%d 分 K" % row.period_minutes)
		item.set_text(4,default_profile if str(row.profile).is_empty() else row.profile)
		if row.id==active_run: item.set_custom_color(0,TEAL)
		if row.id==selected_id: item.select(0)

func _stored() -> Dictionary:
	for row: Dictionary in configurations:
		if row.id==selected_id: return row
	return {}

func _select_combination() -> void:
	var item := combinations.get_selected()
	if item==null or loading or running: return
	var id: String=item.get_metadata(0)
	if id==selected_id: return
	if changed:
		config_message.text="請先儲存或還原目前的搭配。"
		_render_list()
		return
	for row: Dictionary in configurations:
		if row.id==id:
			_show_combination(row)
			return

func _show_combination(row: Dictionary) -> void:
	loading=true
	selected_id=row.get("id","")
	_pick(strategy_picker,row.get("strategy",""))
	_pick(product_picker,row.get("product","TXF"))
	_pick(profiles,row.get("profile",""))
	period_minutes.set_value_no_signal(int(row.get("period_minutes",1)))
	_load_inputs(row.get("inputs",{}),int(row.get("warmup_bars",50)) if row.has("warmup_bars") else -1)
	_update_month()
	changed=false
	config_message.text=""
	details.text="啟用前請核對策略、商品與帳戶。"
	_render_list()
	loading=false
	_sync()

func _add_combination() -> void:
	if running: return
	if changed:
		config_message.text="請先儲存或還原目前的搭配。"
		return
	_show_combination({})
	config_message.text="選擇策略與商品後儲存搭配。"

func _edited() -> void:
	if not loading:
		changed=true
		config_message.text="有尚未儲存的變更。"
	_sync()

func _persist(next: Array) -> bool:
	var stored := ConfigFile.new()
	stored.set_value("execution","rows",next)
	var result := stored.save(CONFIG_PATH)
	if result!=OK:
		config_message.text="無法儲存執行搭配。"
		return false
	configurations=next
	return true

func _save_combination() -> void:
	if running or _strategy().is_empty(): return
	var inputs := {}
	for key in input_fields:
		var value: String=input_fields[key].text.strip_edges()
		if not value.is_valid_float() or not is_finite(float(value)):
			config_message.text="參數 %s 須為數字。" % key
			return
		inputs[key]=float(value)
	var id := selected_id
	if id.is_empty(): id=Crypto.new().generate_random_bytes(12).hex_encode()
	var row := {"id":id,"strategy":_strategy(),"profile":_profile(),"product":_product(),
		"period_minutes":int(period_minutes.value),"warmup_bars":int(warmup_bars.value),"inputs":inputs}
	var next := configurations.duplicate()
	var previous := _stored()
	if previous.is_empty(): next.append(row)
	else: next[next.find(previous)]=row
	if not _persist(next): return
	_show_combination(row)
	config_message.text="已儲存搭配。"

func _delete_combination() -> void:
	if running: return
	var previous := _stored()
	if changed:
		_show_combination(previous)
		return
	if previous.is_empty(): return
	var next := configurations.duplicate()
	next.erase(previous)
	if _persist(next):
		_show_combination(next[0] if not next.is_empty() else {})
		config_message.text="已移除搭配。"

func _set_run_state(label: String) -> void:
	if active_run.is_empty(): return
	execution_states[active_run]=label
	_render_list()
	phase.text=label

func _update_month() -> void:
	if _product() not in LIVE_PRODUCTS:
		month_info.text="此商品可儲存搭配，目前未開放正式自動執行。"
	elif bridge!=null:
		var data: Variant=JSON.parse_string(bridge.nearest_contract(_product()))
		if not data is Dictionary or not data.get("supported",false):
			month_info.text="暫時無法讀取連續月狀態。"
		elif data.get("paused",false):
			month_info.text="結算暫停，等待夜盤行情接續。"
		elif data.get("ambiguous",false):
			month_info.text="有多個即時來源，等待唯一相符的行情。"
		elif str(data.get("instrument","")).is_empty():
			month_info.text="等待此商品的即時行情。"
		else:
			month_info.text="即時行情已就緒。"
	_sync()

func _strategy() -> String:
	return _selected(strategy_picker)

func _product() -> String:
	return _selected(product_picker)

func _profile() -> String:
	return _selected(profiles)

func _enable_blockers() -> PackedStringArray:
	var blockers := PackedStringArray()
	if bridge==null: blockers.append("即時策略模組未載入，請重新開啟工作台。")
	if _strategy().is_empty(): blockers.append("請先選擇策略。")
	if not strategy_message.text.is_empty(): blockers.append("請確認策略可以載入。")
	if selected_id.is_empty() or changed: blockers.append("請先儲存執行搭配。")
	if _product() not in LIVE_PRODUCTS: blockers.append("此商品目前未開放正式自動執行。")
	if _profile().is_empty(): blockers.append("請選擇正式 TradeCom 帳戶設定。")
	elif _profile_needs_rebind(): blockers.append("原交易帳戶與目前選擇不同，請儲存搭配以確認變更。")
	if _product().is_empty(): blockers.append("請填寫下單商品簡碼。")
	var live_path := OS.get_environment("STRATEGY_LIVE_MARKET_DB")
	if live_path.is_empty(): live_path="data/market-live.sqlite"
	if live_path.is_relative_path(): live_path=str(workbench.root).path_join(live_path)
	if not FileAccess.file_exists(live_path):
		blockers.append("找不到即時行情庫。策略可先啟用，開始評估前會等待 QuoteCom。")
	return blockers

func _sync() -> void:
	if not running: phase.text=execution_states.get(selected_id,"已關閉")
	switch.disabled=stopping or (not running and (bridge==null or _strategy().is_empty() or _profile().is_empty()
		or selected_id.is_empty() or changed or _profile_needs_rebind() or not strategy_message.text.is_empty() or _product() not in LIVE_PRODUCTS))
	replay_button.text="停止 225M 模擬" if running and active_run.is_empty() else "225M 自動模擬"
	replay_button.disabled=stopping or bridge==null or (running and not active_run.is_empty())
	if running and active_run.is_empty(): switch.disabled=true
	search.editable=not running
	combinations.mouse_filter=MOUSE_FILTER_IGNORE if running else MOUSE_FILTER_STOP
	combinations.focus_mode=FOCUS_NONE if running else FOCUS_ALL
	add_button.disabled=running
	save_button.disabled=running or _strategy().is_empty() or (not changed and not selected_id.is_empty() and not _profile_needs_rebind())
	delete_button.disabled=running or (selected_id.is_empty() and not changed)
	delete_button.text="還原變更" if changed else "移除搭配"
	strategy_picker.disabled=running
	product_picker.disabled=running
	period_minutes.editable=not running
	warmup_bars.editable=not running
	for field in input_fields.values(): field.editable=not running
	profiles.disabled=running or profiles.item_count<=1
	var blockers := _enable_blockers() if not running else PackedStringArray()
	enable_checks.visible=not blockers.is_empty()
	enable_checks.text="\n".join(blockers)

func _toggle(enabled: bool) -> void:
	if not enabled:
		stop()
		return
	switch.set_pressed_no_signal(false)
	_update_month()
	if running or switch.disabled: return
	pending={"confirmed":true,"id":selected_id,"strategy":_strategy(),"profile":_profile(),
		"period_minutes":int(period_minutes.value),"warmup_bars":int(warmup_bars.value),
		"inputs":_stored().get("inputs",{}),"product":_product()}
	var name: String=execution_names[_strategy()]
	confirmation.dialog_text="策略：%s\n商品：%s 連續月\n週期：%d 分 K\n帳戶設定：%s\n符合條件時會送出正式單口委託。" % [name,pending.product,pending.period_minutes,pending.profile]
	confirmation.popup_centered(Vector2i(530,245))

func _start() -> void:
	if running or pending.is_empty(): return
	_update_month()
	if pending.id!=selected_id or changed or pending.profile!=_profile() or pending.strategy!=_strategy():
		pending={}
		details.text="執行搭配已變更，請重新確認。"
		return
	if not bridge.start(JSON.stringify(pending)): return
	active_run=pending.id
	pending={}
	running=true
	switch.set_pressed_no_signal(true)
	_set_run_state("正在啟用")
	details.text="正在檢查策略與即時行情…"
	_sync()

func stop() -> void:
	if not running or stopping: return
	stopping=true
	switch.set_pressed_no_signal(false)
	bridge.stop()
	_set_run_state("正在停止")
	_sync()

func _process(_delta: float) -> void:
	if not running: return
	var raw: String=bridge.poll()
	if raw.is_empty(): return
	var message: Variant=JSON.parse_string(raw)
	if not message is Dictionary: return
	if message.type=="progress":
		if not stopping:
			if str(message.message).begins_with("已啟用"): _set_run_state("已啟用")
			details.text=message.message
		return
	running=false
	stopping=false
	if message.get("paper",false):
		replay_proof=message.get("proof",{})
		details.text=message.message
		if not replay_proof.is_empty():
			details.text+=" 委託 %d 筆，成交 %d 筆，期末部位 %d 口。\n%s" % [
				replay_proof.order_count,replay_proof.fill_count,replay_proof.final_position,replay_proof.report_path]
			if not replay_proof.get("rollovers",[]).is_empty():
				details.text+="\n已核對 %d 次行情接續。" % replay_proof.rollovers.size()
		_sync()
		return
	switch.set_pressed_no_signal(false)
	_set_run_state("已停止" if message.stopped else "停止待檢查")
	active_run=""
	details.text=message.message
	_sync()
