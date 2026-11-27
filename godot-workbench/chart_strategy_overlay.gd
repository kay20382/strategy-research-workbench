extends RefCounted

const DT = preload("res://date_format.gd")

static func _seconds(value: String) -> int:
	var stamp := DT.canonical_datetime(value).left(19)
	return Time.get_unix_time_from_datetime_string(stamp + "T00:00:00" if stamp.length()==10 else stamp)

static func _tick_time(value: String) -> String:
	var stamp := DT.canonical_datetime(value)
	var nanos := stamp.substr(20) if stamp.length()>19 and stamp[19]=="." else ""
	return stamp.left(19) + "." + nanos.rpad(9,"0")

static func match_fills(fills: Array, times: PackedStringArray, prices: PackedFloat64Array, data: Dictionary) -> Dictionary:
	var marks := {}
	if fills.is_empty() or times.is_empty():
		return marks
	var kind: String = data.kind
	var amount := int(data.amount)
	var clock := PackedInt64Array()
	var direct := {}
	var daily := kind=="daily"
	var tick := kind=="tick"
	var group := int(data.get("overview_group",1))>1
	var start_labeled: bool = kind=="minute" and amount==1 and data.get("role")=="start"
	var span := amount*(60 if kind=="minute" else 1)
	var groups: Array = data.get("bars",[])
	for index in times.size():
		if tick:
			var key := _tick_time(times[index])
			if not direct.has(key): direct[key]=[]
			direct[key].append(index)
		else:
			var seconds := _seconds(times[index])
			clock.append(seconds+span if group and start_labeled else seconds)
			if daily: direct[floori(float(seconds)/(86400*amount))]=index
	for fill: Dictionary in fills:
		var time: String = fill.datetime
		var index := -1
		if tick:
			var trades: Array = direct.get(_tick_time(time), [])
			var matching := trades.filter(func(row): return is_equal_approx(prices[row*5+3],float(fill.price)))
			if matching.size()==1: index=int(matching[0])
		elif group:
			var at := _seconds(time)
			if daily:
				at=_seconds(DT.canonical_datetime(time).left(10))
			elif not start_labeled and _tick_time(time).right(9)!="000000000": at+=1
			index=clock.bsearch(at,not start_labeled)
			if index>=groups.size() or groups[index].size()<8: continue
			var lower := _seconds(groups[index][7])
			if daily or start_labeled:
				if at<lower: continue
			elif lower-at>=span:
				continue
		elif daily:
			index=int(direct.get(floori(float(_seconds(time))/(86400*amount)),-1))
		else:
			if time.length()<19: continue
			var at := _seconds(time)
			if not start_labeled and _tick_time(time).right(9)!="000000000": at+=1
			index=clock.bsearch(at,not start_labeled)-(1 if start_labeled else 0)
			if index<0 or index>=clock.size() or absi(clock[index]-at)>=span:
				continue
		if index>=0:
			if not marks.has(index): marks[index]=[]
			marks[index].append(fill)
	return marks
