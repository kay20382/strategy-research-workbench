extends ConfirmationDialog

signal configuration_applied(config: Dictionary)
var pages: TabContainer
var palette_controls: Dictionary = {}
var period_control: OptionButton
var name_control: LineEdit
var file_control: LineEdit
var from_control: LineEdit
var to_control: LineEdit
var auto_control: CheckBox
var import_picker: FileDialog

func _ready() -> void:
	title="圖表設定"
	initial_position=Window.WINDOW_INITIAL_POSITION_CENTER_MAIN_WINDOW_SCREEN
	size=Vector2i(780,600)
	min_size=Vector2i(680,540)
	ok_button_text="套用設定"
	cancel_button_text="取消"
	pages=TabContainer.new(); pages.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	pages.offset_left=18; pages.offset_top=16; pages.offset_right=-18; pages.offset_bottom=-60
	add_child(pages)
	var product := _page("商品")
	_heading(product,"BROKER CONNECTION", "資料源與商品")
	var provider := OptionButton.new(); provider.add_item("凱基期貨 API")
	_field(product,"券商 API",provider)
	var status := Label.new(); status.text="● 未連線  /  等待凱基報價 SDK 與帳戶設定"
	status.add_theme_color_override("font_color",Color("e6b665")); product.add_child(status)
	var note := Label.new(); note.text="目前圖表從本機日線檔載入；不會以示範資料代替券商行情。\n分 K 資料尚未提供，先使用日／週／月 K 線。"
	note.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART; product.add_child(note)
	name_control=LineEdit.new(); _field(product,"商品名稱",name_control)
	file_control=LineEdit.new(); file_control.editable=false; _field(product,"目前資料檔",file_control)
	var pick := Button.new(); pick.text="選擇本機行情檔（TXT / CSV）"
	pick.pressed.connect(func(): import_picker.popup_centered_ratio(0.65)); product.add_child(pick)
	var settings := _page("設定")
	_heading(settings,"DATA WINDOW", "週期與資料範圍")
	period_control=OptionButton.new()
	for item in ["1 日","1 週","1 月"]: period_control.add_item(item)
	_field(settings,"K 線週期",period_control)
	from_control=LineEdit.new(); from_control.placeholder_text="YYYY-MM-DD，空白表示最早資料"
	_field(settings,"開始日期",from_control)
	to_control=LineEdit.new(); to_control.placeholder_text="YYYY-MM-DD，空白表示最新資料"
	_field(settings,"結束日期",to_control)
	var range_note := Label.new(); range_note.text="保留資料檔原始日期與數值；週線依 ISO 週、月線依日曆月彙整。\n歷史區間不補值，分 K 需另行匯入較細週期資料。"
	range_note.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART; settings.add_child(range_note)
	var style_page := _page("樣式")
	_heading(style_page,"MAKE IT YOURS", "K 線配色")
	for entry in [["up","上漲實體"],["down","下跌實體"],["wick","上下影線"],["background","圖表背景"],["grid","格線"]]:
		var picker := ColorPickerButton.new(); picker.custom_minimum_size=Vector2(120,34); picker.edit_alpha=false
		palette_controls[entry[0]]=picker; _field(style_page,entry[1],picker)
	var reset := Button.new(); reset.text="復原預設配色"
	reset.pressed.connect(func(): _set_palette({"up":"ff5d73","down":"2ed5ba","wick":"b7c6d9","background":"0b101b","grid":"1b2738"}))
	style_page.add_child(reset)
	var scale_page := _page("座標")
	_heading(scale_page,"SMART SCALE", "自適應價格座標")
	auto_control=CheckBox.new(); auto_control.text="套用時復原自適應價格範圍"; auto_control.button_pressed=true
	scale_page.add_child(auto_control)
	var scale_note := Label.new(); scale_note.text="自適應依目前可見 K 棒的最高／最低價計算，保留上下留白。\n\n主畫面工具列可調整：\n↔ 水平縮放　↕ 垂直縮放　↑↓ 價格範圍上移／下移\n\n拖曳右側價格軸也會切換為手動座標。\n點「復原自適應」僅重設價格軸，保留水平縮放與瀏覽位置。"
	scale_note.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART; scale_page.add_child(scale_note)
	var volume := _page("量能概況")
	_heading(volume,"COMING LATER", "量能概況")
	var placeholder := Label.new(); placeholder.text="此頁籤先保留，尚未實作。\n\n目前不提供成交分布、價值區間或買賣量分類。"
	volume.add_child(placeholder)
	import_picker=FileDialog.new(); import_picker.access=FileDialog.ACCESS_FILESYSTEM
	import_picker.file_mode=FileDialog.FILE_MODE_OPEN_FILE
	import_picker.filters=PackedStringArray(["*.txt,*.csv ; 日線行情檔"])
	import_picker.file_selected.connect(func(path): file_control.text=path)
	add_child(import_picker)
	confirmed.connect(_apply)

func _page(label: String) -> VBoxContainer:
	var margin := MarginContainer.new(); margin.name=label
	for edge in ["left","right","top","bottom"]: margin.add_theme_constant_override("margin_"+edge,18)
	pages.add_child(margin)
	var box := VBoxContainer.new(); box.add_theme_constant_override("separation",15); margin.add_child(box)
	return box

func _heading(parent: Node, eyebrow: String, text: String) -> void:
	var e := Label.new(); e.text=eyebrow; e.add_theme_font_size_override("font_size",11)
	e.add_theme_color_override("font_color",Color("6ac9e5")); parent.add_child(e)
	var h := Label.new(); h.text=text; h.add_theme_font_size_override("font_size",23); parent.add_child(h)

func _field(parent: Node, label: String, control: Control) -> void:
	var row := HBoxContainer.new(); row.add_theme_constant_override("separation",16); parent.add_child(row)
	var l := Label.new(); l.text=label; l.custom_minimum_size.x=114; row.add_child(l)
	control.size_flags_horizontal=Control.SIZE_EXPAND_FILL; row.add_child(control)

func _set_palette(colors: Dictionary) -> void:
	for key in palette_controls: palette_controls[key].color=Color(colors[key])

func open_config(config: Dictionary, tab_index := 0) -> void:
	name_control.text=config.instrument
	file_control.text=config.path
	period_control.select(config.period)
	from_control.text=config.get("from",""); to_control.text=config.get("to","")
	auto_control.button_pressed=config.get("auto",true)
	_set_palette(config.palette)
	pages.current_tab=tab_index
	popup_centered(Vector2i(780,600))

func _apply() -> void:
	var colors := {}
	for key in palette_controls: colors[key]=palette_controls[key].color.to_html(false)
	configuration_applied.emit({"instrument":name_control.text.strip_edges(),"path":file_control.text,"period":period_control.selected,"from":from_control.text.strip_edges(),"to":to_control.text.strip_edges(),"auto":auto_control.button_pressed,"palette":colors})
