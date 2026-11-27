extends Control
const DT = preload("res://date_format.gd")

var points: Array = []
var full_count := 0
var hover_x := -1.0
var font: Font

func _ready() -> void:
	custom_minimum_size.y=180
	font=SystemFont.new()
	font.font_names=PackedStringArray(["Microsoft JhengHei UI"])
	mouse_exited.connect(func(): hover_x=-1; queue_redraw())

func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseMotion: hover_x=event.position.x; queue_redraw()

func _draw() -> void:
	var area := Rect2(82,18,maxf(10,size.x-110),maxf(10,size.y-67))
	if points.is_empty():
		draw_string(font,Vector2(24,110),"執行回測後，顯示引擎實際權益曲線",HORIZONTAL_ALIGNMENT_LEFT,-1,18,Color("c0cee3"))
		return
	var lo := float(points[0].equity)
	var hi := lo
	for p in points: lo=minf(lo,float(p.equity)); hi=maxf(hi,float(p.equity))
	var span := maxf(1.0,hi-lo)
	lo-=span*0.12; hi+=span*0.12
	span=hi-lo
	for i in range(5):
		var y := area.position.y+area.size.y*i/4.0
		draw_line(Vector2(area.position.x,y),Vector2(area.end.x,y),Color("24354c"),1)
		draw_string(font,Vector2(2,y+5),"%.0f" % (hi-span*i/4.0),HORIZONTAL_ALIGNMENT_LEFT,77,12,Color("c0cee3"))
	var line := PackedVector2Array()
	for i in range(points.size()): line.append(Vector2(area.position.x+area.size.x*i/maxf(1,points.size()-1),area.end.y-(float(points[i].equity)-lo)/span*area.size.y))
	var color := Color("64f5d2")
	var poly := line.duplicate()
	poly.append(Vector2(area.end.x,area.end.y)); poly.append(Vector2(area.position.x,area.end.y))
	if poly.size()>3: draw_colored_polygon(poly,Color(0.18,0.8,0.7,0.08))
	if line.size()>1:
		draw_polyline(line,Color(0.25,0.9,0.8,0.12),7,true)
		draw_polyline(line,color,2,true)
	draw_string(font,Vector2(area.position.x,size.y-21),DT.date_time(points[0].datetime),HORIZONTAL_ALIGNMENT_LEFT,-1,14,Color("c0cee3"))
	draw_string(font,Vector2(maxf(area.position.x,area.end.x-180),size.y-21),DT.date_time(points[-1].datetime),HORIZONTAL_ALIGNMENT_LEFT,-1,14,Color("c0cee3"))
	if hover_x>=area.position.x and hover_x<=area.end.x:
		var index := clampi(roundi((hover_x-area.position.x)/area.size.x*(points.size()-1)),0,points.size()-1)
		var pos := line[index]
		draw_line(Vector2(pos.x,area.position.y),Vector2(pos.x,area.end.y),Color("80a9ff"),1)
		draw_circle(pos,4,color)
		var label := "%s  權益 %.2f" % [DT.date_time(points[index].datetime),points[index].equity]
		draw_string(font,Vector2(clampf(pos.x-110,area.position.x,maxf(area.position.x,area.end.x-360)),size.y-3),label,HORIZONTAL_ALIGNMENT_LEFT,-1,13,color)
