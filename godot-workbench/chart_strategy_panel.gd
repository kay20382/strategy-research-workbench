extends "res://skin.gd"

signal overlay_changed
signal new_strategy_requested

const CACHE_PATH := "user://chart-strategies.cfg"

var workbench: Control
var row: HBoxContainer
var status_parent: Control
var market_context: Callable
var picker: OptionButton
var new_button: Button
var run_button: Button
var message: Label
var symbol := ""
var choice := {}
var source_hash := ""
var fills: Array = []
var zone := ""
var report_loaded := false
var cache := ConfigFile.new()
var bridge: RefCounted
var run_stage := ""
var run_market := {}
var run_key := ""
var report_bridge: RefCounted
var desired_key := ""
var selected_receipt := {}
var load_key := ""
var loaded_key := ""

func _ready() -> void:
	row=HBoxContainer.new()
	row.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	row.add_theme_constant_override("separation",6)
	add_child(row)
	row.minimum_size_changed.connect(update_minimum_size)
	_label(row,"圖表策略",14,MUTED)
	picker=OptionButton.new()
	picker.custom_minimum_size.x=184
	picker.fit_to_longest_item=false
	row.add_child(picker)
	picker.item_selected.connect(func(_index): _strategy_selected())
	new_button=_button(row,"新增策略",func(): new_strategy_requested.emit())
	run_button=_button(row,"回測並顯示",_run)
	run_button.tooltip_text="用目前圖表的商品、週期與日期回測，完成後顯示進出場。"
	message=_label(status_parent,"",14,MUTED)
	message.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	message.hide()
	cache.load(CACHE_PATH)
	if ClassDB.class_exists("WorkbenchBridge"):
		bridge=ClassDB.instantiate("WorkbenchBridge")
		report_bridge=ClassDB.instantiate("WorkbenchBridge")
	workbench.operation_finished.connect(_workbench_finished)
	reload()

func _set_message(value: String) -> void:
	message.text=value
	message.visible=not value.is_empty()

func _get_minimum_size() -> Vector2:
	return row.get_combined_minimum_size() if row!=null else Vector2.ZERO

func _workbench_finished(operation: String, _data: Variant, error: String) -> void:
	if not error.is_empty(): return
	if operation in ["init","save"]:
		reload()
	elif operation in ["run","run_controlled"]:
		_resolve_report()

func reload() -> void:
	var selected: String=choice.get("id","")
	picker.clear()
	picker.add_item("不顯示策略")
	picker.set_item_metadata(0,{})
	var names := {}
	for entry: Dictionary in workbench.catalog:
		if entry.kind=="strategy": names[entry.name]=int(names.get(entry.name,0))+1
	for entry: Dictionary in workbench.catalog:
		if entry.kind!="strategy": continue
		var label: String=entry.name.trim_prefix("REA_")
		if names[entry.name]>1: label+="（%s）" % entry.id
		picker.add_item(label)
		var index := picker.item_count-1
		picker.set_item_metadata(index,entry)
		if entry.id==selected: picker.select(index)
	if run_stage.is_empty(): _strategy_selected()

func set_symbol(value: String) -> void:
	if symbol==value: return
	symbol=value
	_resolve_report()

func _strategy_selected() -> void:
	choice=picker.get_item_metadata(picker.selected)
	var path: String=choice.get("source","")
	source_hash=""
	if not path.is_empty() and FileAccess.file_exists(path):
		source_hash=FileAccess.get_file_as_string(path).trim_prefix("\uFEFF").sha256_text()
	_resolve_report()

func _receipt_key(market: String) -> String:
	return JSON.stringify([choice.get("id",""),market,source_hash]).sha256_text()

func _valid_receipt(receipt: Variant) -> bool:
	return receipt is Dictionary and receipt.get("status")=="completed" and not source_hash.is_empty() \
		and receipt.get("source_sha256")==source_hash and str(receipt.get("dataset",{}).get("symbol","")).nocasecmp_to(symbol)==0

func _current_receipt() -> Dictionary:
	var path: String=cache.get_value("reports",_receipt_key(symbol),"")
	var best: Dictionary={}
	if not path.is_empty() and FileAccess.file_exists(path):
		var saved: Variant=JSON.parse_string(FileAccess.get_file_as_string(path))
		if _valid_receipt(saved): best=saved
	for document: Dictionary in workbench.documents:
		if document.get("managed_id")!=choice.id and document.get("origin")!=choice.source:
			continue
		var receipt: Variant=document.get("result")
		if _valid_receipt(receipt) and str(receipt.get("started_unix_ns",""))>=str(best.get("started_unix_ns","")):
			best=receipt
	return best

func _resolve_report() -> void:
	run_button.disabled=bridge==null or choice.is_empty() or symbol.is_empty() or not run_stage.is_empty()
	var valid := not choice.is_empty() and not symbol.is_empty() and not source_hash.is_empty()
	selected_receipt=_current_receipt() if valid else {}
	desired_key=_receipt_key(symbol)+str(selected_receipt.get("run_id","")) if valid else ""
	if not desired_key.is_empty() and (desired_key==loaded_key or desired_key==load_key):
		return
	loaded_key=""
	fills=[]
	zone=""
	report_loaded=false
	overlay_changed.emit()
	if not valid:
		_set_message("" if choice.is_empty() or symbol.is_empty() else "無法讀取此策略的原碼。")
		return
	_start_report_load()

func _start_report_load() -> void:
	if report_bridge==null or desired_key.is_empty() or not load_key.is_empty():
		return
	var input := {"root":workbench.root,"action":"chart_strategy_report","symbol":symbol,
		"name":choice.name,"source_hash":source_hash,"receipt":selected_receipt}
	if report_bridge.start(JSON.stringify(input)):
		load_key=desired_key
		_set_message("正在尋找回測進出場…")
	else:
		_set_message("無法啟動回測報表讀取。")

func _run() -> void:
	if run_button.disabled: return
	run_market=market_context.call()
	if run_market.kind=="tick":
		_set_message("圖表回測需要 K 棒，請選擇秒 K、分 K 或日 K。")
		return
	if run_market.kind.is_empty():
		_set_message("正在判定圖表週期，請等行情載入或手動選擇週期。")
		return
	run_key=_receipt_key(run_market.symbol)
	if not _start({"action":"open","entry":choice}):
		_set_message("無法開啟策略。")
		return
	_set_message("正在讀取策略設定…")

func _start(payload: Dictionary) -> bool:
	payload.root=workbench.root
	if not bridge.start(JSON.stringify(payload)): return false
	run_stage=payload.action
	run_button.disabled=true
	picker.disabled=true
	return true

func _run_done(response: Dictionary) -> void:
	var stage := run_stage
	run_stage=""
	picker.disabled=false
	run_button.disabled=choice.is_empty() or symbol.is_empty()
	if not response.get("ok",false):
		_set_message(str(response.get("error","無法執行回測。")))
		return
	if stage=="open":
		var document: Dictionary=response.data
		if _start({"action":"run_controlled","document":document,"chart_market":run_market}):
			_set_message("正在回測 %s 的歷史行情…" % run_market.symbol)
		else:
			_set_message("無法啟動歷史回測。")
		return
	var receipt: Dictionary=response.data.get("receipt",{})
	if receipt.get("status")!="completed":
		_set_message(str(receipt.get("error","回測未完成。")))
		return
	var result_symbol: String=receipt.get("dataset",{}).get("symbol","")
	if not result_symbol.is_empty() and receipt.get("result_path") is String:
		cache.set_value("reports",run_key,receipt.result_path)
		cache.save(CACHE_PATH)
	_resolve_report()
	if result_symbol.nocasecmp_to(symbol)!=0:
		_set_message("%s 的回測已完成。切回該商品可查看進出場。" % result_symbol)

func _process(_delta: float) -> void:
	if not run_stage.is_empty():
		var raw: String=bridge.poll()
		if not raw.is_empty():
			var response: Variant=JSON.parse_string(raw)
			if response is Dictionary and response.get("type")!="log": _run_done(response)
	if load_key.is_empty(): return
	var report_raw: String=report_bridge.poll()
	if report_raw.is_empty(): return
	var finished_key := load_key
	load_key=""
	if finished_key!=desired_key:
		_start_report_load()
		return
	loaded_key=finished_key
	var response: Variant=JSON.parse_string(report_raw)
	if not response is Dictionary or not response.get("ok",false):
		_set_message(str(response.get("error","本次回測報表無法讀取。")) if response is Dictionary else "本次回測報表無法讀取。")
		return
	var data: Variant=response.get("data")
	if not data is Dictionary:
		_set_message("此策略與商品尚無可用的回測結果。")
		return
	fills=data.fills
	zone=data.zone if data.zone is String else ""
	report_loaded=true
	_set_message("回測 %s · %d 筆成交" % [data.run_id,fills.size()])
	overlay_changed.emit()

func _exit_tree() -> void:
	if not run_stage.is_empty(): bridge.stop_controlled_run()
