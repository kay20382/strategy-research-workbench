extends SceneTree
const DT = preload("res://date_format.gd")
func _initialize() -> void:
	assert(DT.date("2026-10-23")=="2026/10/23")
	assert(DT.time("05:00")=="05:00:00")
	assert(DT.date_time("2026-10-23T02:34:00.123456789")=="2026/10/23 02:34:00")
	assert(DT.canonical_datetime("2026/10/23 02:34:00")=="2026-10-23T02:34:00")
	var original := {"from":"2026-10-23T02:34:00.123456789","path":"C:/data/2026-10-23.csv"}
	var shown: Dictionary=DT.presentation(original)
	assert(shown.from=="2026/10/23 02:34:00")
	assert(original.from=="2026-10-23T02:34:00.123456789")
	assert(shown.path==original.path)
	print("DATE_FORMAT_OK")
	quit(0)
