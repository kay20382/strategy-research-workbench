extends Control

const INK = Color("f5f8ff")
const MUTED = Color("c0cee3")
const TEAL = Color("64f5d2")
const BLUE = Color("aec9ff")
var buttons: Array[Button] = []

func _style(color: Color, border: Color = Color("23334e"), radius: int = 12) -> StyleBoxFlat:
	var box := StyleBoxFlat.new()
	box.bg_color = color
	box.border_color = border
	box.set_border_width_all(1)
	box.set_corner_radius_all(radius)
	box.content_margin_left = 18
	box.content_margin_right = 18
	box.content_margin_top = 14
	box.content_margin_bottom = 14
	return box

func _theme() -> void:
	var t := Theme.new()
	var font := SystemFont.new()
	font.font_names = PackedStringArray(["Microsoft JhengHei UI","Microsoft JhengHei"])
	t.default_font = font
	t.default_font_size = 16
	for cls in ["Label","Button","LineEdit","OptionButton","CheckBox","ItemList","RichTextLabel","TabContainer","TabBar"]:
		t.set_color("font_color",cls,INK)
		t.set_color("font_disabled_color",cls,MUTED)
	for cls in ["Button","OptionButton"]:
		for state in ["normal","hover","pressed","focus","disabled"]:
			var color := Color("15243b")
			if state == "hover": color = Color("233f5a")
			if state == "pressed": color = Color("1c5360")
			var b := _style(color,TEAL if state == "focus" else Color("2b405c"),8)
			b.content_margin_top=9; b.content_margin_bottom=9
			t.set_stylebox(state,cls,b)
	for state in ["normal","focus","read_only"]:
		var b := _style(Color("0c1526"),TEAL if state == "focus" else Color("253650"),7)
		b.content_margin_top=9; b.content_margin_bottom=9
		t.set_stylebox(state,"LineEdit",b)
	t.set_stylebox("panel","TabContainer",_style(Color("0e192b")))
	t.set_stylebox("tab_selected","TabContainer",_style(Color("243d56"),TEAL,7))
	t.set_stylebox("tab_unselected","TabContainer",_style(Color("101d31")))
	t.set_color("font_selected_color","TabContainer",TEAL)
	t.set_color("font_unselected_color","TabContainer",INK)
	t.set_color("font_placeholder_color","LineEdit",Color("becde3"))
	t.set_color("font_uneditable_color","LineEdit",Color("d5dfed"))
	t.set_stylebox("panel","ItemList",_style(Color("0c1627")))
	t.set_stylebox("selected","ItemList",_style(Color("1e4450"),TEAL,8))
	t.set_stylebox("selected_focus","ItemList",_style(Color("1e4450"),TEAL,8))
	t.set_constant("v_separation","ItemList",16)
	t.set_color("font_selected_color","ItemList",INK)
	t.set_stylebox("panel","PopupMenu",_style(Color("16253c")))
	t.set_stylebox("panel","AcceptDialog",_style(Color("16253c")))
	t.set_stylebox("background","ProgressBar",_style(Color("16243b")))
	t.set_stylebox("fill","ProgressBar",_style(TEAL))
	theme=t

func _label(parent: Node, text: String, font_size: int = 14, color: Color = INK) -> Label:
	var l := Label.new()
	l.text=text
	l.add_theme_font_size_override("font_size",maxi(14,font_size))
	l.add_theme_color_override("font_color",color)
	parent.add_child(l)
	return l

func _button(parent: Node,text: String,callback: Callable, primary: bool = false) -> Button:
	var b := Button.new()
	b.text=text
	b.size_flags_vertical=SIZE_SHRINK_CENTER
	if primary:
		b.add_theme_stylebox_override("normal",_style(Color("18534f"),Color("318779"),8))
		b.add_theme_color_override("font_color",TEAL)
	b.pressed.connect(callback)
	parent.add_child(b)
	buttons.append(b)
	return b

func _panel(parent: Node) -> VBoxContainer:
	var p := PanelContainer.new()
	p.add_theme_stylebox_override("panel",_style(Color("101d31")))
	p.size_flags_horizontal=SIZE_EXPAND_FILL
	parent.add_child(p)
	var v := VBoxContainer.new()
	v.add_theme_constant_override("separation",12)
	p.add_child(v)
	return v

func _row(parent: Node) -> HBoxContainer:
	var h := HBoxContainer.new()
	h.add_theme_constant_override("separation",12)
	parent.add_child(h)
	return h

func _field(parent: Node,caption: String,value: String = "") -> LineEdit:
	var v := VBoxContainer.new()
	v.size_flags_horizontal=SIZE_EXPAND_FILL
	parent.add_child(v)
	_label(v,caption,12,MUTED)
	var f := LineEdit.new()
	f.text=value
	f.size_flags_horizontal=SIZE_EXPAND_FILL
	v.add_child(f)
	return f

