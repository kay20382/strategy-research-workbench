extends "res://skin.gd"

signal new_strategy_requested

const Chart = preload("res://trade_chart.gd")
const StrategyPanel = preload("res://chart_strategy_panel.gd")
const Overlay = preload("res://chart_strategy_overlay.gd")
const DT = preload("res://date_format.gd")
const DatePicker = preload("res://date_picker.gd")
const UNITS := ["auto", "tick", "second", "minute", "daily"]
const PERIOD_LIMITS := {"second":60, "minute":240, "daily":30}

var bridge: RefCounted
var workbench: Control
var strategy_panel: Control
var profile_api: RefCounted
var timer: Timer
var market: OptionButton
var unit: OptionButton
var period: SpinBox
var period_values := {"second":1, "minute":1, "daily":1}
var start_field: HBoxContainer
var end_field: HBoxContainer
var range_error: Label
var start_date := ""
var end_date := ""
var graph: Control
var title: Label
var path_label: Label
var quote: Label
var status: Label
var path := ""
var history_path := ""
var pending_request: Array = []
var last_snapshot := ""
var source_codes: Array = []
var product_names := {}
var broker_profile := ""
var historical_view := false
var displayed_unit := ""
var shown_data := {}

func _ready() -> void:
	set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	_use_compact_theme()
	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(PRESET_FULL_RECT)
	for edge in ["left","right","top","bottom"]:
		margin.add_theme_constant_override("margin_"+edge,12 if edge in ["left","right"] else 8)
	add_child(margin)
	var layout := VBoxContainer.new()
	layout.add_theme_constant_override("separation",6)
	margin.add_child(layout)
	var top := _row(layout)
	title=_label(top,"即時 K 線",21,INK)
	path_label=_label(top,"",14,MUTED)
	path_label.size_flags_horizontal=SIZE_EXPAND_FILL
	path_label.horizontal_alignment=HORIZONTAL_ALIGNMENT_RIGHT
	path_label.vertical_alignment=VERTICAL_ALIGNMENT_CENTER
	path_label.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS

	var tools := HFlowContainer.new()
	tools.add_theme_constant_override("h_separation",12)
	tools.add_theme_constant_override("v_separation",6)
	layout.add_child(tools)
	market=_select(_tool_group(tools),"商品",218)
	var cycle := _tool_group(tools)
	unit=_select(cycle,"週期",78)
	for item in ["自動","Tick","秒 K","分 K","日 K"]:
		unit.add_item(item)
	period=SpinBox.new()
	period.custom_minimum_size.x=64
	period.tooltip_text="週期數"
	period.min_value=1
	period.step=1
	period.set_value_no_signal(1)
	cycle.add_child(period)
	period.hide()
	unit.item_selected.connect(func(_index): _change_period_unit())
	period.value_changed.connect(func(value: float):
		period_values[UNITS[unit.selected]]=int(value)
		_switch_context())

	var dates := _tool_group(tools)
	start_field=_date_field(dates,"起日")
	end_field=_date_field(dates,"迄日")
	var apply := _button(dates,"套用日期",_apply_dates)
	var apply_style := _style(Color("174c49"),Color("2d6a66"),7)
	apply_style.content_margin_top=5
	apply_style.content_margin_bottom=5
	apply_style.content_margin_left=10
	apply_style.content_margin_right=10
	apply.add_theme_stylebox_override("normal",apply_style)
	_button(dates,"清除日期",_clear_dates)
	range_error=_label(layout,"",14,Color("ff8197"))
	range_error.hide()

	market.item_selected.connect(func(_index): _switch_context())
	strategy_panel=StrategyPanel.new()
	strategy_panel.workbench=workbench
	strategy_panel.status_parent=layout
	strategy_panel.market_context=_strategy_market_context
	tools.add_child(strategy_panel)
	strategy_panel.new_strategy_requested.connect(func(): new_strategy_requested.emit())
	strategy_panel.overlay_changed.connect(func():
		if not shown_data.is_empty(): _update(shown_data))

	var chart_column := VBoxContainer.new()
	chart_column.add_theme_constant_override("separation",0)
	chart_column.size_flags_vertical=SIZE_EXPAND_FILL
	layout.add_child(chart_column)
	var quote_strip := PanelContainer.new()
	var strip_style := StyleBoxFlat.new()
	strip_style.bg_color=Color("101d2e")
	strip_style.content_margin_left=9
	strip_style.content_margin_right=9
	strip_style.content_margin_top=3
	strip_style.content_margin_bottom=3
	quote_strip.add_theme_stylebox_override("panel",strip_style)
	chart_column.add_child(quote_strip)
	quote=_label(quote_strip,"",14,BLUE)
	quote.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS
	graph=Chart.new()
	graph.custom_minimum_size=Vector2(420,300)
	graph.size_flags_vertical=SIZE_EXPAND_FILL
	graph.bar_selected.connect(_inspect)
	graph.bar_hovered.connect(_hover)
	chart_column.add_child(graph)

	status=_label(layout,"",14,MUTED)
	status.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	if ClassDB.class_exists("LiveChartBridge"): bridge=ClassDB.instantiate("LiveChartBridge")
	if ClassDB.class_exists("ApiProfileBridge"): profile_api=ClassDB.instantiate("ApiProfileBridge")
	timer=Timer.new()
	timer.wait_time=1.0
	timer.timeout.connect(_refresh)
	add_child(timer)
	var custom := OS.get_environment("STRATEGY_LIVE_MARKET_DB")
	var historical := OS.get_environment("STRATEGY_HISTORICAL_MARKET_DB")
	history_path=historical if not historical.is_empty() else "data/market.sqlite"
	path=custom if not custom.is_empty() else "data/market-live.sqlite"
	path_label.text="歷史 %s  ·  即時 %s" % [history_path.get_file(),path.get_file()]
	path_label.tooltip_text="歷史 %s\n即時 %s" % [history_path,path]
	visibility_changed.connect(_sync_visibility)
	_sync_visibility()

func _use_compact_theme() -> void:
	var compact := Theme.new()
	compact.default_font=get_theme_default_font()
	compact.default_font_size=15
	for type in ["Button","OptionButton","LineEdit"]:
		var states := ["normal","focus","read_only"] if type=="LineEdit" else ["normal","hover","pressed","focus","disabled"]
		for state in states:
			var style: StyleBox=get_theme_stylebox(state,type).duplicate()
			style.content_margin_top=5
			style.content_margin_bottom=5
			style.content_margin_left=10
			style.content_margin_right=10
			compact.set_stylebox(state,type,style)
	theme=compact

func _tool_group(parent: Node) -> HBoxContainer:
	var group := HBoxContainer.new()
	group.add_theme_constant_override("separation",6)
	parent.add_child(group)
	return group

func _select(parent: Node, caption: String, width: int) -> OptionButton:
	_label(parent,caption,14,MUTED)
	var button := OptionButton.new()
	button.custom_minimum_size.x=width
	button.fit_to_longest_item=false
	parent.add_child(button)
	return button

func _date_field(parent: Node, caption: String) -> HBoxContainer:
	_label(parent,caption,14,MUTED)
	var field := DatePicker.new()
	field.text_submitted.connect(func(_text): _apply_dates())
	parent.add_child(field)
	return field

func _apply_dates() -> void:
	var from := DatePicker.canonical_date(start_field.text)
	var to := DatePicker.canonical_date(end_field.text)
	var error := ""
	if (not start_field.text.strip_edges().is_empty() and from.is_empty()) or (not end_field.text.strip_edges().is_empty() and to.is_empty()):
		error="請輸入有效日期（YYYY/MM/DD）。"
	elif not from.is_empty() and not to.is_empty() and from>to:
		error="起日不得晚於迄日。"
	range_error.text=error
	range_error.visible=not error.is_empty()
	if not error.is_empty(): return
	start_date=from
	end_date=to
	start_field.text=DT.date(from)
	end_field.text=DT.date(to)
	_switch_context()
	if is_visible_in_tree(): _sync_visibility()

func _clear_dates() -> void:
	start_field.clear()
	end_field.clear()
	_apply_dates()

func _range_text() -> String:
	if start_date.is_empty() and end_date.is_empty(): return ""
	return " · 顯示期間 %s ～ %s" % [DT.date(start_date) if not start_date.is_empty() else "不限",DT.date(end_date) if not end_date.is_empty() else "最新"]

func reload_profiles() -> void:
	broker_profile=""
	if profile_api!=null:
		var root := ProjectSettings.globalize_path("res://..").simplify_path()
		var response: Variant=JSON.parse_string(profile_api.request(root,"api_profile_list","{}"))
		if response is Dictionary and response.get("ok",false):
			var trading_profiles := PackedStringArray()
			for row in response.data.profiles:
				var info: Dictionary=row.profile
				if info.provider=="kgi_tradecom" and info.purpose=="trade": trading_profiles.append(info.id)
			if trading_profiles.size()==1: broker_profile=trading_profiles[0]
	_refresh()

func _sync_visibility() -> void:
	if is_visible_in_tree():
		var past := not end_date.is_empty() and end_date<Time.get_date_string_from_system()
		if past: timer.stop()
		else: timer.start()
		_refresh()
	else: timer.stop()

func _switch_context() -> void:
	last_snapshot=""
	shown_data={}
	strategy_panel.set_symbol(_key(market))
	graph.limit=0
	graph.selected=-1
	graph.queue_redraw()
	quote.text=""
	quote.tooltip_text=""
	title.text="即時 K 線  ·  %s  ·  %s" % [_key(market), _interval(UNITS[unit.selected], int(period.value))]
	status.text="正在讀取商品與行情…"
	_refresh()

func _change_period_unit() -> void:
	var kind: String=UNITS[unit.selected]
	period.visible=PERIOD_LIMITS.has(kind)
	if PERIOD_LIMITS.has(kind):
		period.set_value_no_signal(1)
		period.max_value=PERIOD_LIMITS[kind]
		period.set_value_no_signal(period_values[kind])
	_switch_context()

func _interval(kind: String, amount: int) -> String:
	match kind:
		"tick": return "Tick"
		"second": return "%d 秒 K" % amount
		"minute": return "%d 分 K" % amount
		"daily": return "%d 日 K" % amount
		_: return "自動週期"

func _key(button: OptionButton) -> String:
	return str(button.get_item_metadata(button.selected)) if button.selected>=0 else ""

func _context() -> Array:
	return [path,history_path,_key(market),broker_profile,UNITS[unit.selected],int(period.value),start_date,end_date]

func _strategy_market_context() -> Dictionary:
	var kind: String=UNITS[unit.selected]
	var amount := int(period.value)
	if kind=="auto":
		kind=str(shown_data.get("kind",""))
		amount=int(shown_data.get("amount",1))
	return {
		"database":history_path, "symbol":_key(market), "kind":kind,
		"period":amount, "from":start_date, "to":end_date, "role":shown_data.get("role","end"),
	}

func _refresh() -> void:
	if bridge==null:
		status.text="即時圖表模組尚未載入。"
		return
	if not is_visible_in_tree() or not pending_request.is_empty(): return
	var context := _context()
	if bridge.start(context[0],context[1],context[2],context[3],"",false,context[4],context[5],context[6],context[7]): pending_request=context

func _process(_delta: float) -> void:
	if pending_request.is_empty(): return
	var raw: String=bridge.poll()
	if raw.is_empty(): return
	var context := pending_request
	pending_request=[]
	if not is_visible_in_tree(): return
	if context!=_context(): _refresh(); return
	var response: Variant=JSON.parse_string(raw)
	if not response is Dictionary or not response.get("ok",false):
		last_snapshot=""
		status.text="即時行情暫時無法讀取 · "+str(response.get("error","") if response is Dictionary else "")
		return
	var incoming: Dictionary=response.data
	var previous := _key(market)
	if source_codes!=incoming.symbols or product_names!=incoming.products:
		source_codes=incoming.symbols
		product_names=incoming.products
		market.clear()
		for code in source_codes:
			var name: String=product_names.get(code,"")
			market.add_item(code if name.is_empty() else "%s · %s" % [code,name])
			market.set_item_metadata(market.item_count-1,code)
		var index := source_codes.find(previous)
		if not source_codes.is_empty(): market.select(maxi(index,0))
		if _key(market)!=previous: _switch_context(); return
	if source_codes.is_empty():
		status.text="兩個資料庫都沒有商品。請在「行情資料庫」建立商品並匯入行情。"
		if not str(incoming.issue).is_empty(): status.text="暫時無法讀取行情資料庫。"
		return
	if incoming.get("data") is Dictionary and not incoming.data.bars.is_empty():
		var state := raw
		if state!=last_snapshot:
			last_snapshot=state
			shown_data=incoming.data
			_update(incoming.data)
			if not str(incoming.issue).is_empty(): status.text+=" · 另一資料來源暫時無法讀取"
	else:
		last_snapshot=""
		shown_data={}
		if graph.limit>0:
			graph.limit=0
			graph.queue_redraw()
			quote.text=""
		var interval := _interval(context[4],context[5])
		title.text="即時 K 線  ·  %s  ·  %s" % [_key(market),interval]
		status.text="這個商品尚無 %s 行情。" % interval if context[4]!="auto" else "這個商品尚無可顯示的行情。"
		status.text+=_range_text()
		if not str(incoming.issue).is_empty(): status.text+=" · 資料來源暫時無法讀取"

func _update(data: Dictionary) -> void:
	var times := PackedStringArray()
	var prices := PackedFloat64Array()
	var marks := {}
	var forming := {}
	for bar in data.bars:
		var index := times.size()
		times.append(str(bar[0]))
		for i in range(1,6): prices.append(float(bar[i]))
		if bar[6]: forming[index]=true
	for index in data.get("fills",{}): marks[int(index)]=data.fills[index]
	var old_count: float=graph.count
	var old_first: float=graph.first
	var had_bars: bool=graph.limit>0
	var old_limit: int=graph.limit
	var full_range := not start_date.is_empty() or not end_date.is_empty()
	var historical: bool=data.source=="history"
	var show_strategy: bool=strategy_panel.report_loaded
	var overview: bool=data.get("tick_overview",false) or int(data.get("overview_group",1))>1
	var zone: String=data.get("zone","")
	var report_zone: String=strategy_panel.zone
	var zones_match: bool=zone in ["","unspecified"] or report_zone in ["","unspecified"] or zone==report_zone
	var strategy_marks := {}
	if show_strategy and zones_match:
		strategy_marks=Overlay.match_fills(strategy_panel.fills,times,prices,data)
		for index in strategy_marks:
			if not marks.has(index): marks[index]=[]
			marks[index]=marks[index]+strategy_marks[index]
	displayed_unit=data.kind
	graph.setup({"times":times,"prices":prices,"fills":marks,"pending":forming,"daily_axis":data.kind=="daily","tick_axis":data.kind=="tick","show_seconds":data.kind in ["tick","second"],"full_range":full_range})
	historical_view=historical
	var interval := _interval(data.kind,int(data.amount))
	if overview: interval+="（全期間概覽）"
	var continuous_product: String=str(data.get("continuous_product",""))
	var chart_name: String=continuous_product+" 連續月" if not continuous_product.is_empty() else _key(market)
	title.text="%s  ·  %s  ·  %s" % ["歷史行情" if historical else "即時 K 線",chart_name,interval]
	var outcome := "行情明細尚未到達"
	if graph.limit>0:
		var last: int=graph.limit-1
		if full_range:
			if had_bars and old_count<old_limit:
				graph.count=minf(old_count,graph._maximum_visible_bars())
				graph.first=clampf(old_first,0,maxf(0,graph.limit-graph.count))
		else:
			if had_bars: graph.count=old_count
			graph.focus_bar(last,true)
		_inspect(last)
		if historical:
			outcome="歷史行情 · 最新資料 %s" % _chart_time(times[last])
			if data.get("range_prefers_history",false): outcome+=" · 清除日期可回到即時行情"
			if data.kind=="minute" and int(data.amount)>1 and data.get("role","")=="unspecified":
				outcome+=" · 來源時間未標記，依收棒時間彙整"
		elif data.kind=="tick":
			outcome="QuoteCom · 最新成交 %s" % _chart_time(times[last])
		else:
			outcome="QuoteCom · 最新棒尾 %s · %s" % [DT.date_time(times[last]),"未收棒" if forming.has(last) else "已收棒"]
		if zone=="Asia/Tokyo": outcome+="（日本時間）"
		if not continuous_product.is_empty(): outcome+=" · 原價接續"
	if int(data.get("unmapped",0))>0: outcome+=" · 部分成交缺少可對齊的時間或方向"
	if data.has("fill_error"): outcome+=" · 成交暫時無法讀取"
	if show_strategy:
		if not zones_match:
			outcome+=" · 回測與行情的時區不同"
		elif strategy_marks.is_empty():
			outcome+=" · 圖表期間內無可對應的策略進出場"
		else:
			outcome+=" · 策略進出場已標示"
	var range_caption := _range_text()
	outcome+=range_caption
	if full_range and not times.is_empty():
		outcome+=" · 資料首末 %s ～ %s" % [DT.date(times[0]),DT.date(times[-1])]
	if data.get("tick_overview",false):
		outcome+=" · Tick 資料量較大，顯示全期間彙整概覽，縮短日期可查看明細"
	elif int(data.get("overview_group",1))>1:
		outcome+=" · 全期間概覽（每 %d 根合成一根），縮短日期可查看明細" % int(data.overview_group)
	status.text=outcome

func _hover(index: int) -> void:
	if index<0: index=graph.selected
	if index<0 or index>=graph.limit: return
	var j := index*5
	var p: PackedFloat64Array=graph.prices
	var stamp := _chart_time(graph.times[index])
	if displayed_unit=="tick":
		quote.text="%s    成交價 %s    量 %s" % [stamp,p[j+3],p[j+4]]
	else:
		quote.text="%s    開 %s    高 %s    低 %s    收 %s    量 %s%s" % [stamp,p[j],p[j+1],p[j+2],p[j+3],p[j+4],"  未收棒" if graph.pending.has(index) else ""]
	quote.tooltip_text=_candle_tooltip(index)

func _chart_time(value: String) -> String:
	if displayed_unit=="daily": return DT.date(value)
	if displayed_unit=="tick" and value.length()>19: return DT.date_time(value)+value.substr(19)
	return DT.date_time(value)

func _inspect(index: int) -> void:
	if index<0 or index>=graph.limit: return
	graph.selected=index
	graph.queue_redraw()
	_hover(index)

func _candle_tooltip(index: int) -> String:
	var lines := PackedStringArray([quote.text])
	var marks: Array=graph.fills.get(index,[])
	for i in mini(marks.size(),12):
		var fill: Dictionary=marks[i]
		var direction := "買進" if fill.side>0 else "賣出"
		if fill.has("is_entry"):
			lines.append("回測 %s · %s %s 口 @ %s" % ["進場" if fill.is_entry else "出場",direction,fill.quantity,fill.price])
			lines.append("%s · %s" % [DT.date_time(fill.datetime),fill.get("label","")])
		else:
			lines.append("%s %s 口 @ %s" % [direction,fill.quantity,fill.price])
			lines.append("%s · 委託 %s · 成交 %s" % [DT.date_time(fill.datetime),fill.order_no,fill.execution_id])
	if marks.size()>12: lines.append("此處共有 %d 筆標記。" % marks.size())
	return "\n".join(lines)
