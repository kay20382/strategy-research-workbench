extends RefCounted

static func date(value: Variant) -> String:
	var s := str(value).strip_edges()
	return s.left(10).replace("-","/") if s.length()>=10 else s

static func time(value: Variant) -> String:
	var s := str(value).strip_edges().replace("T"," ")
	if s.length()>10: s=s.substr(11)
	if s.length()==5: s+=":00"
	return s.left(8)

static func date_time(value: Variant) -> String:
	var s := str(value).strip_edges()
	return date(s)+" "+time(s) if s.length()>10 else date(s)

static func canonical_date(value: String) -> String:
	return value.strip_edges().replace("/","-")

static func canonical_datetime(value: String) -> String:
	return canonical_date(value).replace(" ","T")

static func presentation(value: Variant) -> Variant:
	if value is Dictionary:
		var result := {}
		for key in value: result[key]=presentation(value[key])
		return result
	if value is Array: return value.map(func(v): return presentation(v))
	if value is String:
		var regex := RegEx.new()
		regex.compile("^\\d{4}[-/]\\d{2}[-/]\\d{2}([T ]\\d{2}:\\d{2}:\\d{2}(\\.\\d+)?)?$")
		if regex.search(value): return date_time(value)
	return value
