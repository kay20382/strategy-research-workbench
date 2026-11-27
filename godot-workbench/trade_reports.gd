extends "res://skin.gd"

const DT = preload("res://date_format.gd")
const ProductSymbols = preload("res://product_symbols.gd")
const MODES := {"paper":"模擬","read_only":"凱基唯讀","live":"凱基正式"}
const KINDS := {"transport_ack":"傳輸收單","order_ack":"券商回覆","order_dispatch":"委託送出",
	"order_report":"委託回報","fill":"成交回報","reply":"處理回覆","status":"連線狀態","recovery":"回報回補"}
const OPERATIONS := {"I":"新單","D":"取消","C":"改量","R":"改價","place":"新單","cancel":"取消",
	"modify_quantity":"改量","modify_price":"改價"}
const DETAIL := {"side":"買賣方向","order_no":"委託號","reference":"委託識別","execution_id":"成交序號",
	"trade_date":"交易日期","time":"券商回報時間","price":"價格","quantity":"口數","cumulative":"累計成交",
	"remaining":"剩餘口數","cnt":"電子單號","web_id":"下單主機","message":"訊息","id":"指令編號",
	"request_id":"請求序號","code":"回報代碼"}

var bridge: RefCounted
var profiles: OptionButton
var modes: OptionButton
var kinds: OptionButton
var search: LineEdit
var rows: Tree
var detail: RichTextLabel
var status: Label
var newest: Button
var older: Button
var newer: Button
var timer: Timer
var cursors: Array[int]=[0]
var shown: Array = []
var last_profiles: Array = []
var current_query := ""
var loading := false
var next_cursor := 0
var active_seq := -1
var requested_profile := ""

func _ready() -> void:
	set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	var margins := MarginContainer.new()
	margins.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	for side in ["left","right","top","bottom"]: margins.add_theme_constant_override("margin_"+side,18)
	add_child(margins)
	var layout := VBoxContainer.new()
	layout.add_theme_constant_override("separation",10)
	margins.add_child(layout)
	var header := _row(layout)
	var title := _label(header,"交易回報",24,INK)
	title.size_flags_horizontal=SIZE_EXPAND_FILL
	_button(header,"重新整理",_refresh)
	var filters := HFlowContainer.new()
	filters.add_theme_constant_override("h_separation",8)
	layout.add_child(filters)
	profiles=_options(filters,"來源設定",[["","全部來源"]])
	modes=_options(filters,"模式",[["","全部模式"],["paper","模擬"],["read_only","凱基唯讀"],["live","凱基正式"]])
	kinds=_options(filters,"回報類型",[["","全部類型"],["order_report","委託"],["fill","成交"],["order_ack","收單與券商回覆"],
		["issue","錯誤與拒絕"],["status","連線"],["recovery","回補"]])
	search=_field(filters,"搜尋","")
	search.placeholder_text="商品、委託號、成交序號、訊息"
	search.custom_minimum_size.x=265
	search.text_submitted.connect(func(_text): _reset())
	var find_button := _button(filters,"搜尋",_reset)
	find_button.size_flags_vertical=SIZE_SHRINK_END
	var body := HSplitContainer.new()
	body.size_flags_vertical=SIZE_EXPAND_FILL
	layout.add_child(body)
	rows=Tree.new()
	rows.columns=7
	rows.hide_root=true
	rows.column_titles_visible=true
	rows.select_mode=Tree.SELECT_ROW
	rows.size_flags_horizontal=SIZE_EXPAND_FILL
	rows.custom_minimum_size.x=690
	var headings := ["收到時間（臺北）","模式／來源","回報類型","商品／方向","委託號","價／口數","狀態"]
	for i in headings.size(): rows.set_column_title(i,headings[i])
	rows.set_column_custom_minimum_width(0,152)
	rows.set_column_custom_minimum_width(2,95)
	rows.set_column_custom_minimum_width(4,65)
	rows.item_selected.connect(_selected)
	body.add_child(rows)
	var side := _panel(body)
	var side_panel := side.get_parent() as Control
	side_panel.custom_minimum_size.x=260
	side_panel.size_flags_horizontal=SIZE_SHRINK_END
	_label(side,"回報明細",18,INK)
	detail=RichTextLabel.new()
	detail.size_flags_vertical=SIZE_EXPAND_FILL
	detail.text="選擇一筆回報查看明細。"
	side.add_child(detail)
	var footer := _row(layout)
	status=_label(footer,"等待交易回報",14,MUTED)
	status.size_flags_horizontal=SIZE_EXPAND_FILL
	newest=_button(footer,"最新",_reset)
	newer=_button(footer,"上一頁",func(): cursors.pop_back(); _refresh())
	older=_button(footer,"下一頁",func(): cursors.append(next_cursor); _refresh())
	if ClassDB.class_exists("TradeReportBridge"): bridge=ClassDB.instantiate("TradeReportBridge")
	timer=Timer.new()
	timer.wait_time=1.0
	timer.timeout.connect(func():
		if cursors.size()==1: _refresh())
	add_child(timer)
	visibility_changed.connect(_visibility)
	_visibility()

func _options(parent: Node, label: String, items: Array) -> OptionButton:
	var box := VBoxContainer.new()
	parent.add_child(box)
	_label(box,label,14,MUTED)
	var button := OptionButton.new()
	for item in items:
		button.add_item(item[1])
		button.set_item_metadata(button.item_count-1,item[0])
	box.add_child(button)
	button.item_selected.connect(func(_i): _reset())
	return button

func focus_profile(profile: String) -> void:
	if profile.is_empty(): return
	requested_profile=profile
	_reset()

func _key(button: OptionButton) -> String:
	return str(button.get_item_metadata(button.selected)) if button.selected>=0 else ""

func _reset() -> void:
	cursors=[0]
	active_seq=-1
	_refresh()

func _visibility() -> void:
	if is_visible_in_tree():
		timer.start()
		_refresh()
	else: timer.stop()

func _filter() -> String:
	var query := search.text.strip_edges()
	if query.to_upper()=="TXF1": query="TXF"
	return JSON.stringify({"profile":_key(profiles) if requested_profile.is_empty() else requested_profile,"mode":_key(modes),
		"kind":_key(kinds),"search":query,"before":cursors.back()})

func _refresh() -> void:
	if bridge==null:
		status.text="交易回報模組尚未載入。"
		return
	if loading or not is_visible_in_tree(): return
	current_query=_filter()
	loading=bridge.start(current_query)
	_buttons()

func _buttons() -> void:
	newest.disabled=cursors.size()==1
	newer.disabled=loading or cursors.size()==1
	older.disabled=loading or next_cursor==0

func _process(_delta: float) -> void:
	if not loading: return
	var result: String=bridge.poll()
	if result.is_empty(): return
	loading=false
	_buttons()
	if not is_visible_in_tree(): return
	if current_query!=_filter(): _refresh(); return
	var parsed: Variant=JSON.parse_string(result)
	if not parsed is Dictionary or not parsed.get("ok",false):
		status.text="回報暫時無法讀取。"
		return
	var response: Dictionary=parsed.data
	if response.profiles!=last_profiles or not requested_profile.is_empty():
		last_profiles=response.profiles
		var previous := _key(profiles) if requested_profile.is_empty() else requested_profile
		profiles.clear()
		profiles.add_item("全部來源")
		profiles.set_item_metadata(0,"")
		var options: Array=last_profiles.duplicate()
		if not previous.is_empty() and not options.has(previous): options.append(previous)
		for name in options:
			profiles.add_item("本機模擬" if name=="paper" else name)
			profiles.set_item_metadata(profiles.item_count-1,name)
			if name==previous: profiles.select(profiles.item_count-1)
		requested_profile=""
	next_cursor=int(response.rows.back().seq) if response.more else 0
	status.text=("目前沒有符合條件的回報。" if response.rows.is_empty() else "第 %s 頁 · %s 筆" % [cursors.size(),response.rows.size()])
	if cursors.size()==1: status.text+=" · 自動更新"
	if response.rows!=shown: _display(response.rows)
	_buttons()

func _value(data: Dictionary, key: String, fallback := "") -> String:
	return str(data[key]).strip_edges() if data.get(key)!=null else fallback

func _direction(data: Dictionary) -> String:
	return {"B":"買進","S":"賣出"}.get(_value(data,"side"),"")

func _summary(record: Dictionary) -> String:
	var data: Dictionary=record.data
	if record.issue:
		return _value(data,"message","未成功 · "+_value(data,"code","待核對"))
	match record.kind:
		"fill": return "已成交"
		"order_report": return OPERATIONS.get(_value(data,"function",_value(data,"action")),"已收到回報")
		"transport_ack": return "傳輸已收單"
		"order_ack": return "券商已回覆"
		"order_dispatch": return "等待券商回覆"
		"recovery": return "回補完成"
	return _value(data,"message","已收到回報")

func _display(value: Array) -> void:
	shown=value
	rows.clear()
	var root := rows.create_item()
	var selected: TreeItem
	for record: Dictionary in shown:
		var line := rows.create_item(root)
		var data: Dictionary=record.data
		var direction := _direction(data)
		var product := ProductSymbols.continuous(_value(data,"symbol","—"))
		var price := _value(data,"price","—")
		var qty := _value(data,"quantity","—")
		var mode: String=MODES.get(record.mode,record.mode)
		var values := [DT.date_time(record.received),mode if record.mode=="paper" else mode+" · "+record.profile,
			KINDS.get(record.kind,record.kind),product+(" · "+direction if not direction.is_empty() else ""),
			_value(data,"order_no","—"),price+" / "+qty if price!="—" or qty!="—" else "—",_summary(record)]
		for index in values.size():
			line.set_text(index,values[index])
			line.set_tooltip_text(index,values[index])
		line.set_metadata(0,record)
		if record.issue: line.set_custom_color(6,Color("ff9da4"))
		elif record.kind=="fill": line.set_custom_color(2,TEAL)
		if int(record.seq)==active_seq: selected=line
	if selected!=null: selected.select(0); _selected()
	else: detail.text="選擇一筆回報查看明細。"

func _selected() -> void:
	var item := rows.get_selected()
	if item==null: return
	var record: Dictionary=item.get_metadata(0)
	active_seq=int(record.seq)
	var lines := PackedStringArray([str(KINDS.get(record.kind,record.kind))+" · "+_summary(record),
		DT.date_time(record.received)+"（臺北）",""])
	var data: Dictionary=record.data
	if data.has("symbol"): lines.append("商品  "+ProductSymbols.continuous(_value(data,"symbol")))
	for key in DETAIL:
		var value := _direction(data) if key=="side" else _value(data,key)
		if not value.is_empty(): lines.append(str(DETAIL[key])+"  "+value)
	detail.text="\n".join(lines)
