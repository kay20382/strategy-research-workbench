extends HBoxContainer

signal text_changed(value: String)
signal text_submitted(value: String)

const MONTH_DAYS := [31,28,31,30,31,30,31,31,30,31,30,31]

var multiple := false
var with_time := false
var default_time := ""
var _edit := LineEdit.new()
var _button := Button.new()
var _popup := PopupPanel.new()
var _year: SpinBox
var _month: OptionButton
var _grid: GridContainer
var _opening := false

var text: String:
	get: return _edit.text
	set(value): _edit.text=value
var editable: bool:
	get: return _edit.editable
	set(value):
		_edit.editable=value
		_button.disabled=not value
var placeholder_text: String:
	get: return _edit.placeholder_text
	set(value): _edit.placeholder_text=value

static func labeled(parent: Node, caption: String, value := "", include_time := false, allow_multiple := false) -> HBoxContainer:
	var column := VBoxContainer.new()
	column.size_flags_horizontal=SIZE_EXPAND_FILL
	parent.add_child(column)
	var label := Label.new()
	label.text=caption
	column.add_child(label)
	var picker := new()
	picker.text=value
	picker.with_time=include_time
	picker.multiple=allow_multiple
	picker.size_flags_horizontal=SIZE_EXPAND_FILL
	column.add_child(picker)
	return picker

func _ready() -> void:
	add_theme_constant_override("separation",4)
	_edit.size_flags_horizontal=SIZE_EXPAND_FILL
	_edit.custom_minimum_size.x=144 if with_time else 94
	_edit.placeholder_text="YYYY/MM/DD HH:mm:ss" if with_time else "YYYY/MM/DD"
	if multiple: _edit.placeholder_text="可選多個日期"
	elif not with_time: _edit.max_length=10
	_edit.text_changed.connect(func(value): text_changed.emit(value))
	_edit.text_submitted.connect(func(value): text_submitted.emit(value))
	add_child(_edit)
	_button.text="日曆"
	_button.tooltip_text="選擇日期"
	_button.pressed.connect(_open)
	add_child(_button)
	add_child(_popup)
	var margin := MarginContainer.new()
	for edge in ["left","right","top","bottom"]: margin.add_theme_constant_override("margin_"+edge,8)
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	_popup.add_child(margin)
	var body := VBoxContainer.new()
	margin.add_child(body)
	var nav := HBoxContainer.new()
	body.add_child(nav)
	_make_button(nav,"‹",func(): _shift(-1)).tooltip_text="上個月"
	_year=SpinBox.new()
	_year.min_value=1
	_year.max_value=9999
	_year.suffix="年"
	nav.add_child(_year)
	_month=OptionButton.new()
	_month.size_flags_horizontal=SIZE_EXPAND_FILL
	for m in range(1,13): _month.add_item("%d 月" % m)
	nav.add_child(_month)
	_make_button(nav,"›",func(): _shift(1)).tooltip_text="下個月"
	_year.value_changed.connect(func(_value): _draw_days())
	_month.item_selected.connect(func(_index): _draw_days())
	_grid=GridContainer.new()
	_grid.columns=7
	_grid.add_theme_constant_override("h_separation",4)
	_grid.add_theme_constant_override("v_separation",3)
	body.add_child(_grid)
	var actions := HBoxContainer.new()
	body.add_child(actions)
	_make_button(actions,"今天",func():
		var now := Time.get_date_dict_from_system()
		_choose(int(now.year),int(now.month),int(now.day)))
	_make_button(actions,"清除",func():
		clear()
		text_changed.emit(text)
		_popup.hide())

func _make_button(parent: Container, label: String, action: Callable) -> Button:
	var button := Button.new()
	button.text=label
	button.pressed.connect(action)
	parent.add_child(button)
	return button

func clear() -> void:
	_edit.clear()

static func days_in_month(year: int, month: int) -> int:
	if month==2 and year%4==0 and (year%100!=0 or year%400==0): return 29
	return MONTH_DAYS[month-1]

static func canonical_date(input: String) -> String:
	var parts := input.strip_edges().replace("/","-").split("-")
	if parts.size()!=3 or parts[0].length()!=4 or parts[1].length()!=2 or parts[2].length()!=2: return ""
	for part in parts:
		for code in part:
			if code<"0" or code>"9": return ""
	var year := int(parts[0])
	var month := int(parts[1])
	var day := int(parts[2])
	if year<1 or month<1 or month>12 or day<1 or day>days_in_month(year,month): return ""
	return "%04d-%02d-%02d" % [year,month,day]

func _open() -> void:
	if not editable: return
	var now := Time.get_date_dict_from_system()
	var values := text.split(",",false)
	var seed := canonical_date(values[-1].strip_edges().left(10)) if not values.is_empty() else ""
	_show_month(int(now.year) if seed.is_empty() else int(seed.left(4)),
		int(now.month) if seed.is_empty() else int(seed.substr(5,2)))
	_popup.popup_centered(Vector2i(338,340))

func _show_month(year: int, month: int) -> void:
	_opening=true
	_year.set_value_no_signal(year)
	_month.select(month-1)
	_opening=false
	_draw_days()

func _shift(delta: int) -> void:
	var index := (int(_year.value)-1)*12+_month.selected+delta
	if index>=0 and index<9999*12:
		_show_month(int(float(index)/12)+1,index%12+1)

func _draw_days() -> void:
	if _opening: return
	for child in _grid.get_children():
		_grid.remove_child(child)
		child.queue_free()
	for weekday in ["一","二","三","四","五","六","日"]:
		var label := Label.new()
		label.text=weekday
		label.horizontal_alignment=HORIZONTAL_ALIGNMENT_CENTER
		_grid.add_child(label)
	var year := int(_year.value)
	var month := _month.selected+1
	var epoch := Time.get_unix_time_from_datetime_dict({"year":year,"month":month,"day":1})
	var weekday := int(Time.get_datetime_dict_from_unix_time(epoch).weekday)
	for unused in range((weekday+6)%7):
		_grid.add_child(Control.new())
	var selected := text.replace("-","/")
	for day in range(1,days_in_month(year,month)+1):
		var button := _make_button(_grid,str(day),_choose.bind(year,month,day))
		button.custom_minimum_size=Vector2(38,30)
		for state in ["normal","hover","pressed","focus"]:
			var box := button.get_theme_stylebox(state).duplicate() as StyleBox
			box.content_margin_left=3
			box.content_margin_right=3
			box.content_margin_top=3
			box.content_margin_bottom=3
			button.add_theme_stylebox_override(state,box)
		button.size_flags_horizontal=SIZE_EXPAND_FILL
		var date := "%04d/%02d/%02d" % [year,month,day]
		button.toggle_mode=true
		button.set_pressed_no_signal(date in selected.split(",",false) if multiple else selected.begins_with(date))

func _choose(year: int, month: int, day: int) -> void:
	if not editable: return
	var date := "%04d/%02d/%02d" % [year,month,day]
	if multiple:
		var dates := PackedStringArray()
		for value in text.split(",",false):
			var normalized := value.strip_edges().replace("-","/")
			if not normalized.is_empty() and normalized not in dates: dates.append(normalized)
		if date in dates: dates.remove_at(dates.find(date))
		else: dates.append(date)
		dates.sort()
		text=",".join(dates)
	else:
		var previous := text.strip_edges()
		var suffix := ""
		if with_time:
			if previous.length()>10: suffix=previous.substr(10).replace("T"," ")
			elif not default_time.is_empty(): suffix=" "+default_time
		text=date+suffix
	text_changed.emit(text)
	if multiple: _draw_days()
	else: _popup.hide()
