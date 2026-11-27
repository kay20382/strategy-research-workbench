extends Window

const Chart = preload("res://trade_chart.gd")
const DT = preload("res://date_format.gd")
const DatePicker = preload("res://date_picker.gd")
const RATES := [1.0,2.0,5.0,10.0]

var canvas: Control
var identity: Label
var quote: Label
var state: Label
var details: RichTextLabel
var start_button: Button
var play_button: Button
var step_button: Button
var finish_button: Button
var previous_button: Button
var next_button: Button
var speed: OptionButton
var date_input: HBoxContainer
var date_button: Button
var timer: Timer
var loader: Thread
var run_id := ""
var wait_trade := {}
var queued_run := {}
var fill_bars: Array = []
var active_bar := -1
var replay_bar := -1
var load_error := ""

func _ready() -> void:
	title="回測交易圖"
	min_size=Vector2i(840,560)
	borderless=true
	transient=true
	exclusive=true
	close_requested.connect(func(): timer.stop(); queued_run={}; hide())
	var background := PanelContainer.new()
	var style := StyleBoxFlat.new()
	style.bg_color=Color("101c2d")
	style.content_margin_left=18
	style.content_margin_right=18
	style.content_margin_top=14
	style.content_margin_bottom=12
	background.add_theme_stylebox_override("panel",style)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(background)
	var layout := VBoxContainer.new()
	layout.add_theme_constant_override("separation",10)
	background.add_child(layout)
	identity=_label(layout,"回測交易圖",22,Color("f3f7ff"))
	var toolbar := HFlowContainer.new()
	toolbar.add_theme_constant_override("h_separation",8)
	layout.add_child(toolbar)
	start_button=_button(toolbar,"選擇回放起點",_choose_start)
	play_button=_button(toolbar,"播放",_toggle_play)
	step_button=_button(toolbar,"下一根 K 棒",_step)
	speed=OptionButton.new()
	for rate in RATES: speed.add_item("%d 根／秒" % rate)
	speed.item_selected.connect(func(_i): timer.wait_time=1.0/RATES[speed.selected])
	toolbar.add_child(speed)
	finish_button=_button(toolbar,"結束回放",_finish)
	date_input=DatePicker.new()
	date_input.with_time=true
	date_input.custom_minimum_size.x=201
	date_input.text_submitted.connect(func(_value): _jump_date())
	toolbar.add_child(date_input)
	date_button=_button(toolbar,"跳至日期",_jump_date)
	_button(toolbar,"關閉",func(): close_requested.emit())
	quote=_label(layout,"",15,Color("bcd8ee"))
	var body := HSplitContainer.new()
	body.size_flags_vertical=Control.SIZE_EXPAND_FILL
	layout.add_child(body)
	canvas=Chart.new()
	canvas.size_flags_horizontal=Control.SIZE_EXPAND_FILL
	canvas.custom_minimum_size=Vector2(420,250)
	canvas.bar_selected.connect(_pick)
	canvas.bar_hovered.connect(_show_quote)
	body.add_child(canvas)
	var side := VBoxContainer.new()
	side.custom_minimum_size.x=255
	side.add_theme_constant_override("separation",10)
	body.add_child(side)
	_label(side,"成交明細",18,Color("f3f7ff"))
	var navigation := HBoxContainer.new()
	side.add_child(navigation)
	previous_button=_button(navigation,"上一成交",func(): _jump_fill(-1))
	next_button=_button(navigation,"下一成交",func(): _jump_fill(1))
	details=RichTextLabel.new()
	details.size_flags_vertical=Control.SIZE_EXPAND_FILL
	side.add_child(details)
	_label(side,"水藍 進場  ·  橙色 出場",14,Color("bcd8ee"))
	_label(side,"▲ 買進  ·  ▼ 賣出",14,Color("bcd8ee"))
	state=_label(layout,"",14,Color("bcd8ee"))
	state.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	timer=Timer.new()
	timer.wait_time=1.0
	timer.timeout.connect(_step)
	add_child(timer)
	_sync()

func _label(parent: Node, caption: String, font_size: int, color: Color) -> Label:
	var node := Label.new()
	node.text=caption
	node.add_theme_font_size_override("font_size",font_size)
	node.add_theme_color_override("font_color",color)
	parent.add_child(node)
	return node

func _button(parent: Node, caption: String, action: Callable) -> Button:
	var node := Button.new()
	node.text=caption
	node.pressed.connect(action)
	parent.add_child(node)
	return node

func open_run(receipt: Dictionary, selection := {}) -> void:
	var key := str(receipt.run_id)
	if loader!=null and key!=run_id:
		queued_run={"receipt":receipt,"selection":selection}
		popup_centered_ratio(0.93)
		return
	queued_run={}
	wait_trade=selection.duplicate()
	if run_id==key and loader==null and load_error.is_empty():
		_apply_selection()
	elif loader==null:
		run_id=key
		replay_bar=-1
		active_bar=-1
		fill_bars=[]
		canvas.choosing_start=false
		load_error=""
		timer.stop()
		canvas.limit=0
		canvas.queue_redraw()
		var artifacts: Dictionary=receipt.get("artifacts",{})
		var dataset: Dictionary=receipt.get("dataset",{})
		identity.text="回測交易圖  ·  "+str(dataset.get("symbol",key))
		date_input.clear()
		quote.text=""
		details.text=""
		loader=Thread.new()
		var code := loader.start(_load_data.bind(str(artifacts.get("bars","")),str(artifacts.get("json",""))))
		if code!=OK:
			loader=null
			load_error="無法啟動回測圖表讀取。"
	_sync()
	popup_centered_ratio(0.93)

func _process(_delta: float) -> void:
	if loader==null or loader.is_alive(): return
	var result: Dictionary=loader.wait_to_finish()
	loader=null
	if not queued_run.is_empty():
		var next_run: Dictionary=queued_run
		open_run(next_run.receipt,next_run.selection)
		return
	if result.has("error"):
		load_error=result.error
		_sync()
		return
	canvas.setup(result)
	fill_bars=result.fill_bars
	if wait_trade.is_empty():
		var initial: int=int(fill_bars.back()) if not fill_bars.is_empty() else canvas.limit-1
		canvas.focus_bar(initial,true)
		_inspect(initial)
	_apply_selection()
	_sync()

func _exit_tree() -> void:
	if loader!=null: loader.wait_to_finish()

static func _load_data(bars_path: String, report_path: String) -> Dictionary:
	if bars_path.is_empty() or report_path.is_empty(): return {"error":"本次回測缺少 K 棒或成交快照。"}
	var input := FileAccess.open(bars_path,FileAccess.READ)
	if input==null: return {"error":"找不到本次回測的 K 棒快照。"}
	var header := input.get_csv_line()
	var columns := PackedInt32Array()
	for name in ["datetime","open","high","low","close","volume"]:
		var column := header.find(name)
		if column<0: return {"error":"本次 K 棒快照缺少欄位 "+name}
		columns.append(column)
	var times := PackedStringArray()
	var prices := PackedFloat64Array()
	while input.get_position()<input.get_length():
		var line := input.get_csv_line()
		if line.size()==1 and line[0].strip_edges().is_empty(): continue
		if line.size()!=header.size(): return {"error":"本次 K 棒快照有不完整的資料列。"}
		times.append(line[columns[0]])
		for i in range(1,6):
			var value := line[columns[i]]
			if not value.is_valid_float() or not is_finite(float(value)):
				return {"error":"本次 K 棒快照有無法辨識的價格或成交量。"}
			prices.append(float(value))
	var report: Variant=JSON.parse_string(FileAccess.get_file_as_string(report_path))
	if not report is Dictionary or not report.get("fills") is Array or times.is_empty() or int(report.get("bar_count",0))!=times.size():
		return {"error":"本次回測的成交與 K 棒快照無法核對。"}
	var grouped := {}
	for fill: Dictionary in report.fills:
		var bar := int(fill.get("bar",-1))
		if bar<0 or bar>=times.size(): return {"error":"本次成交回報有對不到 K 棒的資料。"}
		if not grouped.has(bar): grouped[bar]=[]
		grouped[bar].append(fill)
	var fill_indices := grouped.keys()
	fill_indices.sort()
	return {"times":times,"prices":prices,"fills":grouped,"fill_bars":fill_indices}

func _apply_selection() -> void:
	if wait_trade.is_empty() or canvas.limit==0: return
	if replay_bar>=0: _finish()
	canvas.trade=wait_trade
	var index := clampi(int(wait_trade.entry_bar),0,canvas.limit-1)
	canvas.focus_bar(index)
	_inspect(index)
	wait_trade={}

func _pick(index: int) -> void:
	if canvas.choosing_start:
		canvas.choosing_start=false
		replay_bar=index
		canvas.limit=index+1
		canvas.focus_bar(index,true)
	_inspect(index)

func _show_quote(hover: int) -> void:
	var index := hover if hover>=0 else active_bar
	if index<0 or index>=canvas.limit: return
	var t: String=canvas.times[index]
	var p: PackedFloat64Array=canvas.prices
	var j := index*5
	quote.text="%s    開 %s    高 %s    低 %s    收 %s    量 %s" % [DT.date_time(t),p[j],p[j+1],p[j+2],p[j+3],p[j+4]]

func _inspect(index: int) -> void:
	active_bar=index
	canvas.selected=index
	canvas.queue_redraw()
	_show_quote(-1)
	var marks: Array=canvas.fills.get(index,[])
	var lines := PackedStringArray([DT.date_time(canvas.times[index]),""])
	if marks.is_empty(): lines.append("這根 K 棒沒有成交。")
	for mark: Dictionary in marks:
		var direction := "買進" if float(mark.side)>0 else "賣出"
		lines.append("%s  ·  %s" % ["進場" if mark.is_entry else "出場",direction])
		lines.append("%s 口 @ %s" % [int(mark.quantity),str(mark.price)])
		lines.append("%s  %s" % [DT.date_time(mark.datetime),str(mark.label)])
		lines.append("")
	if not canvas.trade.is_empty() and int(canvas.trade.exit_bar)<canvas.limit:
		lines.append("選取交易 #%d · 淨損益 %s" % [int(canvas.trade.id),str(canvas.trade.net)])
		lines.append("進場 %s · 出場 %s" % [str(canvas.trade.entry_price),str(canvas.trade.exit_price)])
	details.text="\n".join(lines)
	_sync()

func _jump_fill(direction: int) -> void:
	var target := fill_bars.bsearch(active_bar,direction<0)+(0 if direction>0 else -1)
	if target<0 or target>=fill_bars.size() or int(fill_bars[target])>=canvas.limit: return
	canvas.focus_bar(int(fill_bars[target]))
	_inspect(int(fill_bars[target]))

func _jump_date() -> void:
	if canvas.limit==0: return
	var requested := DT.canonical_datetime(date_input.text)
	var pattern := RegEx.new()
	pattern.compile("^\\d{4}-\\d{2}-\\d{2}(T([01]\\d|2[0-3]):[0-5]\\d(:[0-5]\\d)?)?$")
	if pattern.search(requested)==null or DatePicker.canonical_date(requested.left(10)).is_empty():
		state.text="請輸入 yyyy/MM/dd 或 yyyy/MM/dd HH:mm。"
		return
	var lo := 0
	var hi: int=canvas.limit
	while lo<hi:
		var mid := (lo+hi)>>1
		if str(canvas.times[mid]).replace(" ","T")<requested: lo=mid+1
		else: hi=mid
	if lo>=canvas.limit or DT.date(canvas.times[lo])!=DT.date(requested):
		state.text="該日期與時間沒有可顯示的 K 棒。"
		return
	canvas.focus_bar(lo)
	if canvas.choosing_start: _pick(lo)
	else: _inspect(lo)

func _choose_start() -> void:
	if replay_bar>=0: _finish()
	canvas.choosing_start=not canvas.choosing_start
	canvas.queue_redraw()
	_sync()

func _toggle_play() -> void:
	if replay_bar<0: return
	if timer.is_stopped(): timer.start()
	else: timer.stop()
	_sync()

func _step() -> void:
	if replay_bar<0 or canvas.limit>=canvas.times.size():
		timer.stop()
		_sync()
		return
	replay_bar+=1
	canvas.limit=replay_bar+1
	canvas.focus_bar(replay_bar,true)
	if canvas.limit==canvas.times.size(): timer.stop()
	_inspect(replay_bar)

func _finish() -> void:
	timer.stop()
	replay_bar=-1
	canvas.choosing_start=false
	canvas.limit=canvas.times.size()
	if active_bar>=0:
		canvas.focus_bar(active_bar)
		_inspect(active_bar)
	_sync()

func _sync() -> void:
	var has_data: bool=canvas.limit>0 and loader==null
	var replay := replay_bar>=0
	start_button.disabled=not has_data
	start_button.text="取消選擇" if canvas.choosing_start else "選擇回放起點"
	step_button.disabled=not replay or canvas.limit>=canvas.times.size()
	play_button.disabled=step_button.disabled
	play_button.text="播放" if timer.is_stopped() else "暫停"
	finish_button.disabled=not replay
	speed.disabled=not has_data
	date_input.editable=has_data
	date_button.disabled=not has_data
	previous_button.disabled=not has_data or fill_bars.bsearch(active_bar,true)==0
	var next := fill_bars.bsearch(active_bar,false)
	next_button.disabled=not has_data or next==fill_bars.size() or int(fill_bars[next])>=canvas.limit
	if not load_error.is_empty(): state.text=load_error
	elif loader!=null: state.text="正在讀取本次回測快照…"
	elif canvas.choosing_start: state.text="點選一根 K 棒，從該棒開始回放。"
	elif replay: state.text="回放 · 第 %d / %d 根 · %s" % [canvas.limit,canvas.times.size(),DT.date_time(canvas.times[replay_bar])]
	else: state.text="%d 根 K 棒 · 滾輪縮放，拖曳平移，點選 K 棒查看成交" % canvas.limit
