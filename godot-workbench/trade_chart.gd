extends Control

signal bar_selected(index: int)
signal bar_hovered(index: int)

const DT = preload("res://date_format.gd")
const UP := Color("45d8b0")
const DOWN := Color("ff8197")
const ENTRY := Color("89dfff")
const EXIT := Color("ffc278")
const GRID := Color("223248")
const TEXT := Color("bbcbe1")

var times := PackedStringArray()
var prices := PackedFloat64Array()
var fills := {}
var pending := {}
var limit := 0
var first := 0.0
var count := 120.0
var selected := -1
var trade := {}
var choosing_start := false
var plot := Rect2()
var mouse := Vector2(-1,-1)
var show_seconds := false
var daily_axis := false
var tick_axis := false
var full_range := false
var lod_step := 0
var lod_limit := 0
var lod_prices := PackedFloat64Array()
var mark_indices: Array = []
var marker_step := 1
var display_marks := {}
var display_mark_indices: Array = []
var gesture := false
var dragged := false
var press_at := Vector2.ZERO
var press_first := 0.0

func _ready() -> void:
	clip_contents=true
	mouse_default_cursor_shape=CURSOR_CROSS
	resized.connect(queue_redraw)
	mouse_exited.connect(func():
		mouse=Vector2(-1,-1)
		bar_hovered.emit(-1)
		queue_redraw())

func setup(data: Dictionary) -> void:
	times=data.times
	prices=data.prices
	fills=data.fills
	pending=data.get("pending",{})
	daily_axis=data.get("daily_axis",false)
	tick_axis=data.get("tick_axis",false)
	full_range=data.get("full_range",false)
	limit=times.size()
	lod_step=0
	lod_prices=PackedFloat64Array()
	mark_indices=fills.keys()
	mark_indices.sort()
	marker_step=1
	display_marks=fills
	display_mark_indices=mark_indices
	show_seconds=data.get("show_seconds",false)
	for i in range(mini(12,limit)):
		if DT.time(times[i]).right(2)!="00": show_seconds=true
	count=maxf(float(limit),12) if full_range else clampf(float(limit),12,120)
	first=maxf(0,limit-count)
	trade={}
	selected=-1
	mouse=Vector2(-1,-1)
	queue_redraw()

func focus_bar(index: int, right := false) -> void:
	first=clampf(index-count*(0.88 if right else 0.5),0,maxf(0,limit-count))
	queue_redraw()

func _at(x: float) -> int:
	if plot.size.x<=0: return -1
	var index := floori(first+(x-plot.position.x)*count/plot.size.x)
	return index if index>=0 and index<limit else -1

func _x(index: int) -> float:
	return plot.position.x+(index+0.5-first)*plot.size.x/count

func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton:
		if event.button_index in [MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN] and event.pressed and plot.has_point(event.position):
			var fraction: float=(event.position.x-plot.position.x)/plot.size.x
			var anchor := first+count*fraction
			count=clampf(count*(0.82 if event.button_index==MOUSE_BUTTON_WHEEL_UP else 1.22),12,_maximum_visible_bars())
			first=clampf(anchor-count*fraction,0,maxf(0,limit-count))
			queue_redraw()
			accept_event()
		elif event.button_index==MOUSE_BUTTON_LEFT:
			if event.pressed and plot.has_point(event.position):
				gesture=true
				dragged=false
				press_first=first
				press_at=event.position
				accept_event()
			elif not event.pressed and gesture:
				gesture=false
				var index := _pick_index(event.position.x)
				if not dragged and plot.has_point(event.position) and index>=0: bar_selected.emit(index)
				accept_event()
	elif event is InputEventMouseMotion:
		mouse=event.position
		if gesture and absf(mouse.x-press_at.x)>4: dragged=true
		if gesture and dragged and plot.size.x>0:
			first=clampf(press_first+(press_at.x-mouse.x)*count/plot.size.x,0,maxf(0,limit-count))
		bar_hovered.emit(_at(mouse.x) if plot.has_point(mouse) else -1)
		queue_redraw()

func _maximum_visible_bars() -> float:
	return maxf(float(limit),12) if full_range else clampf(float(limit),12,450)

func _pick_index(x: float) -> int:
	var index := _at(x)
	if not full_range or index<0 or fills.has(index) or display_mark_indices.is_empty(): return index
	var position := display_mark_indices.bsearch(index)
	var best := 7.0
	for i in [position-1,position]:
		if i<0 or i>=display_mark_indices.size(): continue
		var candidate: int=display_mark_indices[i]
		var distance := absf(_x(candidate)-x)
		if distance<best:
			index=candidate
			best=distance
	return index

func _prepare_markers(step: int) -> void:
	if marker_step==step: return
	marker_step=step
	if step<=1:
		display_marks=fills
		display_mark_indices=mark_indices
		return
	display_marks={}
	display_mark_indices=[]
	var current := -1
	var used := 0
	for index: int in mark_indices:
		var group := index/step
		if group!=current:
			current=group
			used=0
		if used==3: continue
		for fill: Dictionary in fills[index]:
			var bit := 1 if fill.get("is_entry",float(fill.side)>0) else 2
			if used&bit: continue
			used|=bit
			if not display_marks.has(index):
				display_marks[index]=[]
				display_mark_indices.append(index)
			display_marks[index].append(fill)

func _draw_prices(stride: int) -> PackedFloat64Array:
	if stride==1: return prices
	if lod_step==stride and lod_limit==limit: return lod_prices
	lod_step=stride
	lod_limit=limit
	lod_prices.resize(ceili(float(limit)/stride)*5)
	for base in range(0,limit,stride):
		var i := base*5
		var j := (base/stride)*5
		var end := mini(base+stride,limit)
		var high:=prices[i+1]
		var low:=prices[i+2]
		var volume:=0.0
		for bar in range(base,end):
			var n := bar*5
			high=maxf(high,prices[n+1])
			low=minf(low,prices[n+2])
			volume+=prices[n+4]
		lod_prices[j]=prices[i]
		lod_prices[j+1]=high
		lod_prices[j+2]=low
		lod_prices[j+3]=prices[(end-1)*5+3]
		lod_prices[j+4]=volume
	return lod_prices

func _draw() -> void:
	draw_rect(Rect2(Vector2.ZERO,size),Color("0b1524"))
	if limit<=0 or size.x<180 or size.y<140: return
	plot=Rect2(14,12,size.x-114,size.y-49)
	var volume_height := minf(93,plot.size.y*0.2)
	var floor_y := plot.end.y-volume_height
	var start := clampi(ceili(first-0.5),0,limit-1)
	var end := mini(limit,ceili(first+count-0.5))
	if start>=end: return
	var stride := 1
	while float(end-start)/stride>maxf(plot.size.x/2.0,1.0): stride*=2
	var draw_prices := _draw_prices(stride)
	var first_group := start/stride
	var last_group := (end-1)/stride
	_prepare_markers(ceili(count*10.0/plot.size.x) if full_range and stride>1 else 1)
	var first_mark := display_mark_indices.bsearch(start)
	var end_mark := display_mark_indices.bsearch(end)
	var low := INF
	var high := -INF
	var volume := 1.0
	for group in range(first_group,last_group+1):
		low=minf(low,draw_prices[group*5+2])
		high=maxf(high,draw_prices[group*5+1])
		volume=maxf(volume,draw_prices[group*5+4])
	for index in range(first_mark,end_mark):
		var i: int=display_mark_indices[index]
		for fill in display_marks[i]:
			low=minf(low,float(fill.price))
			high=maxf(high,float(fill.price))
	var pad := maxf((high-low)*0.09,maxf(absf(high)*0.00005,0.000001))
	low-=pad
	high+=pad
	var value_y := func(value: float) -> float:
		return lerpf(floor_y-7,plot.position.y+7,(value-low)/(high-low))
	var font := get_theme_default_font()
	draw_rect(Rect2(plot.end.x,0,size.x-plot.end.x,size.y),Color("101d2e"))
	draw_rect(Rect2(plot.position.x,floor_y,plot.size.x,volume_height),Color("101a2b"))
	var rough_step := (high-low)/5.0
	var magnitude := pow(10.0,floorf(log(rough_step)/log(10.0)))
	var step := magnitude*10.0
	for multiple in [1.0,2.0,2.5,5.0,10.0]:
		if multiple*magnitude>=rough_step:
			step=multiple*magnitude
			break
	for tick in range(ceili(low/step),floori(high/step)+1):
		var value := tick*step
		var y: float=value_y.call(value)
		draw_line(Vector2(plot.position.x,y),Vector2(plot.end.x,y),GRID)
		draw_string(font,Vector2(plot.end.x+9,y+5),String.num(value,5),HORIZONTAL_ALIGNMENT_LEFT,85,13,TEXT)
	var grid_step := maxi(1,ceili(count/maxf(1.0,plot.size.x/142.0)))
	var long_axis := full_range and DT.date(times[start]).left(4)!=DT.date(times[end-1]).left(4)
	for i in range(start+posmod(-start,grid_step),end,grid_step):
		var x := _x(i)
		draw_line(Vector2(x,plot.position.y),Vector2(x,plot.end.y),GRID)
		if x<plot.end.x-104:
			var tick_label := DT.date(times[i]) if daily_axis or long_axis else DT.date(times[i]).substr(5)+" "+DT.time(times[i]).left(8 if show_seconds else 5)
			draw_string(font,Vector2(x+4,plot.end.y+24),tick_label,HORIZONTAL_ALIGNMENT_LEFT,110,13,TEXT)
	var width := minf(15,plot.size.x/count*stride*0.68)
	for group in range(first_group,last_group+1):
		var j := group*5
		var base := group*stride
		var tail := mini(base+stride-1,limit-1)
		var x := (_x(maxi(start,base))+_x(mini(end-1,tail)))/2.0
		var color := UP if draw_prices[j+3]>=draw_prices[j] else DOWN
		if pending.has(tail): color.a=0.68
		var o: float=value_y.call(draw_prices[j])
		var c: float=value_y.call(draw_prices[j+3])
		if tick_axis and stride==1:
			if group>0 and draw_prices[j+3]<draw_prices[j-2]: color=DOWN
			draw_circle(Vector2(x,c),clampf(width/3,1.3,3.5),color)
		else:
			draw_line(Vector2(x,value_y.call(draw_prices[j+1])),Vector2(x,value_y.call(draw_prices[j+2])),color,1.2)
			draw_rect(Rect2(x-width/2,minf(o,c),width,maxf(1,absf(c-o))),color)
		var h := draw_prices[j+4]/volume*(volume_height-8)
		draw_rect(Rect2(x-width/2,plot.end.y-h,width,h),Color(color,0.5))
	if not trade.is_empty() and int(trade.exit_bar)<limit:
		var entry_x := _x(int(trade.entry_bar))
		var exit_x := _x(int(trade.exit_bar))
		var a := maxf(plot.position.x,entry_x)
		var b := minf(plot.end.x,exit_x)
		if a<=b:
			var y1 := clampf(value_y.call(float(trade.entry_price)),plot.position.y,floor_y)
			var y2 := clampf(value_y.call(float(trade.exit_price)),plot.position.y,floor_y)
			var span := maxf(0.001,exit_x-entry_x)
			draw_dashed_line(Vector2(a,lerpf(y1,y2,(a-entry_x)/span)),Vector2(b,lerpf(y1,y2,(b-entry_x)/span)),Color("c5b8ff"),1.5,5)
	for index in range(first_mark,end_mark):
		var i: int=display_mark_indices[index]
		var marks: Array=display_marks[i]
		if marks.is_empty(): continue
		var x := _x(i)
		for k in range(mini(4,marks.size())):
			var mark: Dictionary=marks[k]
			var y := clampf(value_y.call(float(mark.price)),plot.position.y+6,floor_y-10)
			var shift := (k-(mini(4,marks.size())-1)*0.5)*5
			if marker_step>1:
				shift+=-2.0 if mark.get("is_entry",float(mark.side)>0) else 2.0
			var pos := Vector2(x+shift,y)
			var buy := float(mark.side)>0
			var direction := 1 if buy else -1
			var shape := PackedVector2Array([pos+Vector2(0,-6*direction),pos+Vector2(-4,3*direction),pos+Vector2(4,3*direction)])
			var mark_color := (ENTRY if mark.is_entry else EXIT) if mark.has("is_entry") else (ENTRY if buy else EXIT)
			draw_colored_polygon(shape,mark_color)
			draw_polyline(PackedVector2Array([shape[0],shape[1],shape[2],shape[0]]),Color("0b1524"),1,true)
	if selected>=start and selected<end:
		var x := _x(selected)
		draw_dashed_line(Vector2(x,plot.position.y),Vector2(x,plot.end.y),Color("6ba4bf"),1,3)
	if plot.has_point(mouse):
		var line_color := Color("90d8ff") if choosing_start else Color("60768e")
		draw_dashed_line(Vector2(mouse.x,plot.position.y),Vector2(mouse.x,plot.end.y),line_color,1,4)
		if mouse.y<floor_y:
			draw_dashed_line(Vector2(plot.position.x,mouse.y),Vector2(plot.end.x,mouse.y),line_color,1,4)
			var value := lerpf(low,high,(floor_y-7-mouse.y)/(floor_y-plot.position.y-14))
			draw_rect(Rect2(plot.end.x,mouse.y-11,size.x-plot.end.x,22),Color("29435c"))
			draw_string(font,Vector2(plot.end.x+9,mouse.y+5),String.num(value,5),HORIZONTAL_ALIGNMENT_LEFT,85,13,Color.WHITE)
