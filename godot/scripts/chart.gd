extends Control

signal bar_hovered(bar: Dictionary)
signal viewport_changed(text: String)
signal tool_finished

const UP = Color("ff515b")
const DOWN = Color("19d59a")
const TEXT = Color("b9c5d0")
const GRID = Color("263039")
const ACCENT = Color("63b5ff")
var bars: Array = []
var visible_count := 150.0
var first := 0.0
var style := 0
var crosshair := true
var show_ma := false
var show_volume := false
var show_roc := false
var up_color := UP
var down_color := DOWN
var wick_color := Color("fff2bc")
var background_color := Color("080c10")
var grid_color := GRID
var auto_scale := true
var scale_ready := false
var fixed_mid := 0.0
var fixed_spread := 1.0
var tool := "cursor"
var drawings: Array = []
var pending: Dictionary = {}
var mouse := Vector2(-1, -1)
var dragging := false
var price_min := 0.0
var price_max := 1.0
var price_scale := 1.0
var price_shift := 0.0
var axis_drag := false
var plot := Rect2()
var volume_plot := Rect2()
var roc_plot := Rect2()
var font: Font
var last_hover := -1

func _ready() -> void:
	clip_contents = true
	mouse_default_cursor_shape = Control.CURSOR_CROSS
	font = ThemeDB.fallback_font
	resized.connect(queue_redraw)
	mouse_exited.connect(func(): mouse = Vector2(-1,-1); last_hover = -1; queue_redraw())

func set_bars(value: Array, reset := true) -> void:
	bars = value
	if reset:
		visible_count = minf(180.0, maxf(12.0, bars.size()))
		first = maxf(0.0, bars.size() - visible_count + 8.0)
		price_scale = 1.0
		price_shift = 0.0
		auto_scale = true
		scale_ready = false
		pending = {}
	last_hover = -1
	queue_redraw()

func set_tool(value: String) -> void:
	tool = value
	pending = {}
	dragging = false
	mouse_default_cursor_shape = Control.CURSOR_DRAG if value == "pan" else Control.CURSOR_CROSS
	queue_redraw()

func reset_view() -> void:
	price_scale = 1.0
	price_shift = 0.0
	auto_scale = true
	visible_count = minf(180.0, maxf(12.0, bars.size()))
	first = maxf(0.0, bars.size() - visible_count + 8.0)
	queue_redraw()

func fit_all() -> void:
	first = 0.0
	visible_count = maxf(12.0, bars.size() + 5.0)
	price_scale = 1.0
	price_shift = 0.0
	auto_scale = true
	queue_redraw()

func restore_auto_scale() -> void:
	auto_scale = true
	price_scale = 1.0
	price_shift = 0.0
	queue_redraw()

func adjust_price_scale(factor: float) -> void:
	if not scale_ready: return
	if auto_scale:
		fixed_mid = (price_min+price_max)/2.0
		fixed_spread = price_max-price_min
		auto_scale = false
	fixed_spread = clampf(fixed_spread*factor,0.00001,1.0e12)
	queue_redraw()

func shift_price(fraction: float) -> void:
	adjust_price_scale(1.0)
	if not auto_scale: fixed_mid += fixed_spread*fraction
	queue_redraw()

func palette() -> Dictionary:
	return {"up":up_color.to_html(false),"down":down_color.to_html(false),"wick":wick_color.to_html(false),"background":background_color.to_html(false),"grid":grid_color.to_html(false)}

func apply_palette(value: Dictionary) -> void:
	up_color=Color(value.get("up","ff515b"))
	down_color=Color(value.get("down","19d59a"))
	wick_color=Color(value.get("wick","fff2bc"))
	background_color=Color(value.get("background","080c10"))
	grid_color=Color(value.get("grid","263039"))
	queue_redraw()

func zoom(factor: float, anchor := 0.5) -> void:
	var old := visible_count
	visible_count = clampf(visible_count * factor, 12.0, maxf(24.0,bars.size() * 1.4))
	first += (old - visible_count) * anchor
	_clamp_view()
	queue_redraw()

func move_bars(amount: float) -> void:
	first += amount
	_clamp_view()
	queue_redraw()

func _clamp_view() -> void:
	first = clampf(first, -visible_count * 0.2, maxf(0.0,bars.size() - visible_count * 0.25))

func _x(index: float) -> float:
	return plot.position.x + (index - first + 0.5) * plot.size.x / visible_count

func _y(price: float) -> float:
	return plot.end.y - (price - price_min) / maxf(0.000001,price_max - price_min) * plot.size.y

func _price(y: float) -> float:
	return price_min + (plot.end.y - y) / plot.size.y * (price_max-price_min)

func _index(x: float) -> float:
	return first + (x-plot.position.x) / plot.size.x * visible_count - 0.5

func _point_to_anchor(pos: Vector2) -> Dictionary:
	var idx := clampi(roundi(_index(pos.x)),0,bars.size()-1)
	return {"date":bars[idx].date,"price":_price(pos.y)}

func _anchor_to_point(anchor: Dictionary) -> Vector2:
	for i in range(bars.size()):
		if bars[i].date == anchor.date:
			return Vector2(_x(i),_y(anchor.price))
	return Vector2(-10000,-10000)

func _gui_input(event: InputEvent) -> void:
	if bars.is_empty(): return
	if event is InputEventMouseButton:
		mouse = event.position
		if event.button_index == MOUSE_BUTTON_WHEEL_UP and event.pressed:
			zoom(0.85, clampf((mouse.x-plot.position.x)/maxf(1,plot.size.x),0,1)); accept_event()
		elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN and event.pressed:
			zoom(1.0/0.85, clampf((mouse.x-plot.position.x)/maxf(1,plot.size.x),0,1)); accept_event()
		elif event.button_index == MOUSE_BUTTON_RIGHT and event.pressed:
			pending = {}; set_tool("cursor"); tool_finished.emit(); accept_event()
		elif event.button_index == MOUSE_BUTTON_LEFT or event.button_index == MOUSE_BUTTON_MIDDLE:
			if not event.pressed:
				dragging = false; axis_drag = false
			elif event.double_click:
				reset_view()
			elif mouse.x >= plot.end.x:
				axis_drag = true
			elif plot.has_point(mouse) and event.button_index == MOUSE_BUTTON_LEFT and tool in ["line","horizontal","rectangle"]:
				var anchor := _point_to_anchor(mouse)
				if tool == "horizontal":
					drawings.append({"kind":tool,"a":anchor,"b":anchor}); tool_finished.emit()
				elif pending.is_empty():
					pending = {"kind":tool,"a":anchor}
				else:
					pending.b = anchor; drawings.append(pending.duplicate(true)); pending = {}; tool_finished.emit()
			else:
				dragging = true
			accept_event()
		queue_redraw()
	elif event is InputEventMouseMotion:
		mouse = event.position
		if axis_drag:
			adjust_price_scale(exp(event.relative.y * 0.008))
		elif dragging:
			first -= event.relative.x / maxf(1,plot.size.x) * visible_count
			_clamp_view()
		var idx := clampi(roundi(_index(mouse.x)),0,bars.size()-1)
		if idx != last_hover:
			last_hover = idx
			bar_hovered.emit(bars[idx])
		queue_redraw()

func _text(pos: Vector2, text: String, color := TEXT, font_size := 12) -> void:
	draw_string(font,pos,text,HORIZONTAL_ALIGNMENT_LEFT,-1,font_size,color)

func _draw() -> void:
	draw_rect(Rect2(Vector2.ZERO,size),background_color)
	var lower_height := (105.0 if show_volume else 0.0) + (110.0 if show_roc else 0.0)
	plot = Rect2(12,42,maxf(50,size.x-110),maxf(90,size.y-76-lower_height))
	volume_plot = Rect2(12,plot.end.y+12,plot.size.x,90)
	roc_plot = Rect2(12,plot.end.y+12+(105 if show_volume else 0),plot.size.x,95)
	if bars.is_empty():
		_text(Vector2(40,90),"請匯入 CSV 或載入示範資料",TEXT,18)
		return
	var start := maxi(0,floori(first))
	var stop := mini(bars.size(),ceili(first+visible_count)+1)
	var lo := INF
	var hi := -INF
	var max_volume := 1.0
	var max_roc := 1.0
	for i in range(start,stop):
		lo = minf(lo,bars[i].low); hi = maxf(hi,bars[i].high)
		max_volume = maxf(max_volume,bars[i].volume)
		if bars[i].get("roc") != null: max_roc = maxf(max_roc,absf(bars[i].roc))
	if lo == INF: return
	var mid := (hi+lo)/2.0
	var spread := maxf(hi-lo,maxf(1.0,absf(mid)*0.005))*1.10
	if not auto_scale:
		mid = fixed_mid
		spread = fixed_spread
	price_min = mid-spread/2.0; price_max = mid+spread/2.0
	scale_ready = true
	for j in range(9):
		var yy := plot.position.y+plot.size.y*j/8.0
		draw_dashed_line(Vector2(plot.position.x,yy),Vector2(plot.end.x,yy),grid_color,1,2)
		_text(Vector2(plot.end.x+10,yy+4),"%.2f" % (price_max-(price_max-price_min)*j/8.0))
	var label_step := maxi(1,roundi(visible_count/8))
	for i in range(start,stop):
		if i % label_step == 0 and _x(i)>=plot.position.x and _x(i)<=plot.end.x:
			var xx := _x(i)
			draw_dashed_line(Vector2(xx,plot.position.y),Vector2(xx,size.y-29),grid_color,1,2)
			_text(Vector2(xx-26,size.y-10),bars[i].date.substr(2),TEXT,11)
	draw_line(Vector2(plot.end.x,25),Vector2(plot.end.x,size.y-28),GRID)
	_text(Vector2(16,24),"OHLC  ·  %d 根" % bars.size(),Color("e2eaf2"),13)
	if show_ma:
		_text(Vector2(170,24),"MA5",Color("ffc466")); _text(Vector2(225,24),"MA20",Color("af94ff")); _text(Vector2(291,24),"MA60",Color("56cce0"))
	var width := clampf(plot.size.x/visible_count*0.68,1,32)
	for i in range(start,stop):
		var b: Dictionary = bars[i]
		var xx := _x(i)
		if xx < plot.position.x or xx > plot.end.x: continue
		var color := up_color if b.close>=b.open else down_color
		if b.high<price_min or b.low>price_max: continue
		var yo := clampf(_y(b.open),plot.position.y,plot.end.y)
		var yc := clampf(_y(b.close),plot.position.y,plot.end.y)
		var yh := clampf(_y(b.high),plot.position.y,plot.end.y)
		var yl := clampf(_y(b.low),plot.position.y,plot.end.y)
		if style == 3:
			if i > start: draw_line(Vector2(_x(i-1),clampf(_y(bars[i-1].close),plot.position.y,plot.end.y)),Vector2(xx,yc),ACCENT,1.5,true)
		else:
			draw_line(Vector2(xx,yh),Vector2(xx,yl),wick_color,1,true)
			if style == 2:
				draw_line(Vector2(xx-width/2,yo),Vector2(xx,yo),color)
				draw_line(Vector2(xx,yc),Vector2(xx+width/2,yc),color)
			else:
				var rect := Rect2(xx-width/2,minf(yo,yc),width,maxf(1,absf(yc-yo)))
				if style==1 and b.close>=b.open: draw_rect(rect,color,false,1)
				else: draw_rect(rect,color)
		if show_volume:
			var height: float = b.volume/max_volume*(volume_plot.size.y-22)
			draw_rect(Rect2(xx-width/2,volume_plot.end.y-height,width,height),Color(color,0.7))
		if show_roc and b.get("roc") != null:
			var zero := roc_plot.get_center().y+8
			var height := float(b.roc)/max_roc*(roc_plot.size.y/2-18)
			draw_rect(Rect2(xx-width/2,minf(zero,zero-height),width,maxf(1,absf(height))),UP if b.roc>=0 else DOWN)
	if show_ma:
		for entry in [["ma5",Color("ffc466")],["ma20",Color("af94ff")],["ma60",Color("56cce0")]]:
			for i in range(maxi(1,start),stop):
				if bars[i].get(entry[0]) == null or bars[i-1].get(entry[0]) == null: continue
				var a := Vector2(_x(i-1),_y(bars[i-1][entry[0]]))
				var b := Vector2(_x(i),_y(bars[i][entry[0]]))
				if plot.has_point(a) and plot.has_point(b): draw_line(a,b,entry[1],1,true)
	if show_volume:
		draw_line(Vector2(12,volume_plot.position.y-5),Vector2(size.x,volume_plot.position.y-5),GRID)
		_text(Vector2(17,volume_plot.position.y+12),"成交量",TEXT,12)
		_text(Vector2(plot.end.x+8,volume_plot.position.y+14),"%.0f"%max_volume,TEXT,11)
	if show_roc:
		draw_line(Vector2(12,roc_plot.position.y-5),Vector2(size.x,roc_plot.position.y-5),GRID)
		draw_line(Vector2(12,roc_plot.get_center().y+8),Vector2(plot.end.x,roc_plot.get_center().y+8),Color("53616d"))
		_text(Vector2(17,roc_plot.position.y+12),"ROC(10)  動能 %",TEXT,12)
		_text(Vector2(plot.end.x+8,roc_plot.get_center().y+12),"0.00",TEXT,11)
	# Drawings are keyed by date and price so zoom/pan never alters their meaning.
	for drawing in drawings: _draw_annotation(drawing)
	if not pending.is_empty() and plot.has_point(mouse):
		var preview := pending.duplicate(true)
		preview.b = _point_to_anchor(mouse)
		_draw_annotation(preview)
	var last: Dictionary = bars.back()
	var last_y := _y(last.close)
	if last_y >= plot.position.y and last_y <= plot.end.y:
		draw_dashed_line(Vector2(12,last_y),Vector2(plot.end.x,last_y),Color(up_color if last.close>=last.open else down_color,0.45),1,4)
		draw_rect(Rect2(plot.end.x+1,last_y-10,96,21),up_color if last.close>=last.open else down_color)
		_text(Vector2(plot.end.x+6,last_y+5),"%.2f"%last.close,Color("080c10"),12)
	if crosshair and mouse.x>=plot.position.x and mouse.x<=plot.end.x and mouse.y>=plot.position.y and mouse.y<size.y-28:
		var xx := _x(roundi(_index(mouse.x)))
		draw_dashed_line(Vector2(xx,plot.position.y),Vector2(xx,size.y-28),Color("aab7c5"),1,4)
		if plot.has_point(mouse):
			draw_dashed_line(Vector2(12,mouse.y),Vector2(plot.end.x,mouse.y),Color("aab7c5"),1,4)
			draw_rect(Rect2(plot.end.x+1,mouse.y-10,96,21),Color("344455"))
			_text(Vector2(plot.end.x+6,mouse.y+5),"%.2f"%_price(mouse.y),Color.WHITE,12)
		var idx := clampi(roundi(_index(mouse.x)),0,bars.size()-1)
		draw_rect(Rect2(clampf(xx-45,12,plot.end.x-90),size.y-28,90,23),Color("344455"))
		_text(Vector2(clampf(xx-42,15,plot.end.x-87),size.y-12),bars[idx].date,Color.WHITE,11)
	viewport_changed.emit("%s  ·  顯示 %d–%d / %d 根"%["座標：自適應" if auto_scale else "座標：手動",start+1,stop,bars.size()])

func _draw_annotation(drawing: Dictionary) -> void:
	var a := _anchor_to_point(drawing.a)
	var b := _anchor_to_point(drawing.b)
	var color := Color("ffcc70")
	if drawing.kind == "horizontal":
		if a.y>=plot.position.y and a.y<=plot.end.y:
			draw_line(Vector2(plot.position.x,a.y),Vector2(plot.end.x,a.y),color,1.5,true)
			_text(Vector2(18,a.y-5),"%.2f"%drawing.a.price,color,11)
	else:
		if drawing.kind == "rectangle":
			var rect := Rect2(a,b-a).abs().intersection(plot)
			if rect.has_area():
				draw_rect(rect,Color(color,0.08)); draw_rect(rect,color,false,1.5)
		else:
			var segment := _clip_line(a,b)
			if segment.size()==2: draw_line(segment[0],segment[1],color,1.5,true)
		if plot.has_point(a): draw_circle(a,3,color)
		if plot.has_point(b): draw_circle(b,3,color)

func _clip_line(a: Vector2, b: Vector2) -> PackedVector2Array:
	var points := PackedVector2Array()
	if plot.has_point(a): points.append(a)
	if plot.has_point(b): points.append(b)
	var corners := [plot.position,Vector2(plot.end.x,plot.position.y),plot.end,Vector2(plot.position.x,plot.end.y)]
	for i in range(4):
		var intersection: Variant = Geometry2D.segment_intersects_segment(a,b,corners[i],corners[(i+1)%4])
		if intersection != null:
			var duplicate := false
			for p in points:
				if p.distance_to(intersection)<0.01: duplicate=true
			if not duplicate: points.append(intersection)
	return points
