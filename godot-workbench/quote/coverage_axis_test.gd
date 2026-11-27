extends SceneTree

const Chart = preload("res://quote/coverage.gd")

func _initialize() -> void:
	var chart := Chart.new()
	chart.size=Vector2(1100,205)
	var rows := [{"kind":"minute","timeline":[
		{"start_ms":1000,"end_ms":2000,"status":"present"},
		{"start_ms":2000,"end_ms":3000,"status":"missing"},
		{"start_ms":10000,"end_ms":12000,"status":"present"}]}]
	chart.set_data(rows,0,20000)
	assert(chart.trading_axis.size()==2)
	assert(chart.trading_duration_ms==4000)
	assert(is_equal_approx(chart._x(3000),chart._x(10000)))
	assert(is_equal_approx(chart._x(5000),chart._x(3000)))
	assert(chart._time_at(0)==1000)
	assert(chart._time_at(0.5)==10000)
	assert(chart._time_at(1)==12000)
	assert(is_equal_approx(chart._x(2000)-chart._x(1000),(chart._x(12000)-chart._x(1000))/4.0))
	# 日期跳躍與多日休市仍使用相同映射，不會按相隔日數留白。
	rows[0].timeline[2].start_ms=604810000
	rows[0].timeline[2].end_ms=604812000
	chart.set_data(rows,0,604820000)
	assert(chart.trading_duration_ms==4000)
	assert(chart._time_at(0.5)==604810000)
	assert(is_equal_approx(chart._x(3000),chart._x(604810000)))
	chart.set_data([{"kind":"minute","timeline":[]}],0,20000)
	assert(chart.trading_axis.is_empty() and chart.trading_duration_ms==0)
	chart.free()
	print("COVERAGE_TRADING_AXIS_OK")
	quit(0)
