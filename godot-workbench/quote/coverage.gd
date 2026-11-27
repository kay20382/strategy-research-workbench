extends Control

const DT = preload("res://date_format.gd")
const GOOD = Color("ffffff")
const MISSING = Color("ff647e")
var rows: Array = []
var timeline_from_ms := 0.0
var timeline_to_ms := 1.0
var font: Font
var trading_axis: Array = []
var trading_duration_ms := 0.0

func set_data(sources: Array, from_ms: float, to_ms: float) -> void:
	rows=sources
	timeline_from_ms=from_ms
	timeline_to_ms=to_ms
	trading_axis=[]
	trading_duration_ms=0.0
	# 1分K時間軸涵蓋所有預期交易分鐘，無論有資料或缺資料。
	for row in rows:
		if row.kind!="minute": continue
		for segment in row.get("timeline",[]):
			var a := maxf(from_ms,float(segment.start_ms))
			var b := minf(to_ms,float(segment.end_ms))
			if b<=a: continue
			if not trading_axis.is_empty() and float(trading_axis[-1].end_ms)==a:
				trading_axis[-1].end_ms=b
			else: trading_axis.append({"start_ms":a,"end_ms":b,"offset_ms":trading_duration_ms})
			trading_duration_ms+=b-a
	queue_redraw()

func _elapsed(value: float) -> float:
	var lo := 0
	var hi := trading_axis.size()
	while lo<hi:
		var mid := (lo+hi)/2
		if float(trading_axis[mid].start_ms)<=value: lo=mid+1
		else: hi=mid
	if lo==0: return 0.0
	var span: Dictionary=trading_axis[lo-1]
	return float(span.offset_ms)+clampf(value-float(span.start_ms),0,float(span.end_ms)-float(span.start_ms))

func _time_at(ratio: float) -> float:
	if trading_axis.is_empty(): return timeline_from_ms
	var value := clampf(ratio,0,1)*trading_duration_ms
	var lo := 0
	var hi := trading_axis.size()
	while lo<hi:
		var mid := (lo+hi)/2
		var span: Dictionary=trading_axis[mid]
		if value>=float(span.offset_ms)+float(span.end_ms)-float(span.start_ms): lo=mid+1
		else: hi=mid
	if lo==trading_axis.size(): return float(trading_axis[-1].end_ms)
	var span: Dictionary=trading_axis[lo]
	return float(span.start_ms)+value-float(span.offset_ms)

func _ready() -> void:
	custom_minimum_size=Vector2(0,205)
	font=SystemFont.new()
	font.font_names=PackedStringArray(["Microsoft JhengHei UI"])
	resized.connect(queue_redraw)

func _plot() -> Rect2:
	return Rect2(78,12,maxf(100,size.x-176),125)

func _x(value: float) -> float:
	var p := _plot()
	return p.position.x+_elapsed(value)/maxf(1,trading_duration_ms)*p.size.x

func _stamp(value: float) -> String:
	return DT.date_time(Time.get_datetime_string_from_unix_time(floori(value/1000.0)))

func _draw() -> void:
	if rows.is_empty():
		draw_string(font,Vector2(10,74),"檢查後以時間軸顯示資料；紅色為缺漏區段",HORIZONTAL_ALIGNMENT_LEFT,-1,16,Color("c0cee3"))
		return
	if trading_duration_ms<=0:
		draw_string(font,Vector2(10,74),"此範圍無交易時段，休盤直接跳過",HORIZONTAL_ALIGNMENT_LEFT,-1,16,Color("c0cee3"))
		return
	var p := _plot()
	var names := {"tick":"Tick","minute":"1分K","daily":"日K"}
	for i in range(rows.size()):
		var row: Dictionary=rows[i]
		var y := p.position.y+i*42.0
		draw_string(font,Vector2(1,y+20),names.get(row.kind,row.kind),HORIZONTAL_ALIGNMENT_LEFT,-1,15,Color("f5f8ff"))
		for segment in row.get("timeline",[]):
			var left := _x(float(segment.start_ms))
			var right := _x(float(segment.end_ms))
			if right<=left: continue
			var color := MISSING if segment.status=="missing" else GOOD
			draw_rect(Rect2(left,y,maxf(0.8,right-left),25),color)
		var suffix: String="%d 段缺口" % int(row.gap_count) if row.kind=="tick" else "缺 %d 棒" % int(row.missing)
		draw_string(font,Vector2(p.end.x+9,y+19),suffix,HORIZONTAL_ALIGNMENT_LEFT,-1,13,MISSING if row.missing>0 else Color("c0cee3"))
	var ticks := 5 if p.size.x>850 else 3
	for i in range(ticks):
		var ratio := float(i)/(ticks-1)
		var x := p.position.x+p.size.x*ratio
		draw_line(Vector2(x,10),Vector2(x,139),Color(0.8,0.87,1,0.18),1)
		var stamp := _stamp(_time_at(ratio))
		var width := font.get_string_size(stamp,HORIZONTAL_ALIGNMENT_LEFT,-1,12).x
		var label_x := clampf(x-width/2,p.position.x,maxf(p.position.x,p.end.x-width))
		draw_string(font,Vector2(label_x,158),stamp,HORIZONTAL_ALIGNMENT_LEFT,-1,12,Color("d5e1f2"))
	draw_rect(Rect2(78,179,12,12),GOOD)
	draw_string(font,Vector2(97,190),"有資料",HORIZONTAL_ALIGNMENT_LEFT,-1,14,Color("ffffff"))
	draw_rect(Rect2(290,179,12,12),MISSING)
	draw_string(font,Vector2(309,190),"缺資料",HORIZONTAL_ALIGNMENT_LEFT,-1,14,Color("ff647e"))
	draw_string(font,Vector2(420,190),"休盤已跳過",HORIZONTAL_ALIGNMENT_LEFT,-1,14,Color("d5e1f2"))

func _get_tooltip(at_position: Vector2) -> String:
	var p := _plot()
	if not p.has_point(at_position) or rows.is_empty() or trading_duration_ms<=0: return ""
	var index := clampi(int((at_position.y-p.position.y)/42),0,rows.size()-1)
	var row: Dictionary=rows[index]
	var t := _time_at((at_position.x-p.position.x)/p.size.x)
	for segment in row.get("timeline",[]):
		if t>=float(segment.start_ms) and t<float(segment.end_ms):
			return ("缺資料" if segment.status=="missing" else "有資料")+"\n"+_stamp(segment.start_ms)+" → "+_stamp(segment.end_ms)+("\nTick交易時段內空白達3分鐘或整段無資料，標記疑似缺漏；未達3分鐘以白色顯示" if row.kind=="tick" else "")
	return "休盤（依商品設定）\n"+_stamp(t)

func _box(color: Color) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color=color
	box.set_corner_radius_all(5)
	return box
