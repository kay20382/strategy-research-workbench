extends RefCounted

static func continuous(symbol: String) -> String:
	var code := symbol.strip_edges().to_upper()
	var fields := code.split(":")
	if fields.size()>=2 and fields[0] in ["JPX","OSE","PAPER"] and fields[1] in ["225M","NK225M","JNM"]:
		return "225M"
	for prefix in ["225M-","NK225M-","JNM-"]:
		if code.begins_with(prefix): return "225M"
	if code.length()==5 and code[3] in "ABCDEFGHIJKL" and code[4].is_valid_int():
		var family := code.left(3)
		if family in ["TXF","MXF","TMF","CDF"]: return family
	return symbol
