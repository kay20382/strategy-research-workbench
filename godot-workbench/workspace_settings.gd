extends "res://skin.gd"

signal saved

const MASK := "******"
const AI = preload("res://ai_api.gd")
var root := ""
var bridge: RefCounted
var model: LineEdit
var model_presets: OptionButton
var model_choices: LineEdit
var provider: OptionButton
var provider_caption: Label
var displayed_source := "openai"
var source_drafts := {}
var saved_sources := {}
var saved_provider := "openai"
var chat_id: LineEdit
var credentials := {}
var edit_button: Button
var save_button: Button
var cancel_button: Button
var status: Label
var editing := false
var key_setup_source := ""
var telegram_state: Label
var telegram_start: Button
var telegram_test: Button
var telegram_stop: Button
var ai_section: Control
var telegram_section: Control

func _ready() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	root = ProjectSettings.globalize_path("res://..").simplify_path()
	var margins := MarginContainer.new()
	margins.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	for side in ["left","right","top","bottom"]:
		margins.add_theme_constant_override("margin_"+side,24)
	add_child(margins)
	var scroll := ScrollContainer.new()
	scroll.horizontal_scroll_mode=ScrollContainer.SCROLL_MODE_DISABLED
	margins.add_child(scroll)
	var layout := VBoxContainer.new()
	layout.size_flags_horizontal=SIZE_EXPAND_FILL
	layout.add_theme_constant_override("separation",16)
	scroll.add_child(layout)
	var tools := _row(layout)
	var spacer := Control.new()
	spacer.size_flags_horizontal=SIZE_EXPAND_FILL
	tools.add_child(spacer)
	edit_button=_button(tools,"修改設定",func(): _set_editing(true),true)
	cancel_button=_button(tools,"取消修改",_reload)
	save_button=_button(tools,"儲存設定",_save,true)

	var ai := _panel(layout)
	ai_section=ai
	var selector := _row(ai)
	provider_caption=_label(selector,"預設 AI 來源",14,MUTED)
	provider=OptionButton.new()
	for name in AI.NAMES.values():
		provider.add_item(name)
	selector.add_child(provider)
	provider.item_selected.connect(_switch_source)
	var models := _row(ai)
	var model_column := VBoxContainer.new()
	model_column.size_flags_horizontal=SIZE_EXPAND_FILL
	models.add_child(model_column)
	_label(model_column,"預設模型",14,MUTED)
	model_presets=OptionButton.new()
	model_presets.fit_to_longest_item=false
	model_presets.item_selected.connect(_select_preset)
	model_column.add_child(model_presets)
	model=_field(model_column,"自訂模型代碼")
	model_choices=_field(models,"額外模型（逗號分隔）")
	model_choices.text_changed.connect(func(_text: String): _refresh_presets())
	_secret_field(ai,"ai_key","API 金鑰")
	_label(ai,"對話首頁與 AI 優化共用這組設定。",14,MUTED)

	var telegram := _panel(layout)
	telegram_section=telegram
	telegram_section.hide()
	chat_id=_field(telegram,"對話（Chat ID）")
	chat_id.placeholder_text="使用者、群組 ID 或 @頻道名稱"
	_secret_field(telegram,"telegram_bot_token","令牌（Bot Token）")
	var telegram_tools := _row(telegram)
	telegram_start=_button(telegram_tools,"開始收訊",func(): telegram_start_requested.emit(false),true)
	telegram_test=_button(telegram_tools,"測試收發",func(): telegram_start_requested.emit(true))
	telegram_stop=_button(telegram_tools,"停止收訊",func(): telegram_stop_requested.emit())
	telegram_stop.disabled=true
	telegram_state=_label(telegram,"Telegram 尚未開始收訊。",14,MUTED)
	telegram_state.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	status=_label(layout,"",14,MUTED)
	status.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	visibility_changed.connect(func():
		if not is_visible_in_tree():
			_hide_credentials())
	if ClassDB.class_exists("ApiProfileBridge"):
		bridge=ClassDB.instantiate("ApiProfileBridge")
	_reload()

func show_section(show_telegram: bool) -> void:
	if telegram_section.visible!=show_telegram:
		_hide_credentials()
	ai_section.visible=not show_telegram
	telegram_section.visible=show_telegram

func _request(action: String, data := {}) -> Dictionary:
	if bridge==null or not bridge.has_method("settings"):
		return {"ok":false,"error":"找不到工作台設定模組。"}
	var response: Variant=JSON.parse_string(bridge.settings(root,action,JSON.stringify(data)))
	return response if response is Dictionary else {"ok":false,"error":"無法讀取工作台設定。"}

func ai_config(source := "") -> Dictionary:
	var response := _request("ai",{"provider":source})
	return response.data if response.ok else {}

func _capture_source() -> void:
	var entry: Dictionary=credentials.ai_key
	var draft := {"model":model.text,"models":_additional_models()}
	var key: String=entry.field.text.strip_edges()
	if entry.clear.button_pressed or not key.is_empty():
		draft.key="" if entry.clear.button_pressed else key
	source_drafts[displayed_source]=draft

func _switch_source(index: int) -> void:
	_capture_source()
	_show_source(AI.NAMES.keys()[index])

func _show_source(source: String) -> void:
	displayed_source=source
	var saved: Dictionary=saved_sources.get(source,{})
	var draft: Dictionary=source_drafts.get(source,saved)
	model.text=draft.get("model","")
	model.placeholder_text=AI.MODELS[source][0]
	model_choices.text=", ".join(draft.get("models",[]))
	_refresh_presets()
	var entry: Dictionary=credentials.ai_key
	_hide_secret(entry)
	entry.stored=saved.get("key_set",false)
	entry.clear.set_pressed_no_signal(draft.has("key") and draft.key.is_empty())
	entry.field.text=draft.get("key","")
	_sync_secret(entry)

func _additional_models() -> Array:
	return Array(model_choices.text.replace("，",",").split(",",false))

func _refresh_presets() -> void:
	var selected_model := model.text.strip_edges()
	model_presets.clear()
	model_presets.add_item("沿用來源預設 · "+AI.MODELS[displayed_source][0])
	model_presets.set_item_metadata(0,"")
	var selected := 0
	for candidate in AI.available_models(displayed_source,_additional_models()):
		var index := model_presets.item_count
		model_presets.add_item(candidate)
		model_presets.set_item_metadata(index,candidate)
		if candidate==selected_model:
			selected=index
	model_presets.add_item("自訂模型…")
	if not selected_model.is_empty() and selected==0:
		selected=model_presets.item_count-1
	model_presets.select(selected)
	model.get_parent().visible=selected==model_presets.item_count-1

func _select_preset(index: int) -> void:
	var custom := index==model_presets.item_count-1
	model.get_parent().visible=custom
	if custom:
		model.grab_focus()
	else:
		model.text=model_presets.get_item_metadata(index)

signal telegram_start_requested(test: bool)
signal telegram_stop_requested

func telegram_config() -> Dictionary:
	var response := _request("telegram")
	return response.data if response.ok else {}

func update_telegram_state(message: String, active: bool) -> void:
	telegram_state.text=message
	telegram_start.disabled=active
	telegram_stop.disabled=not active

func _secret_field(panel: Node, key: String, title: String) -> void:
	var row := _row(panel)
	var field := _field(row,title)
	field.secret=true
	field.secret_character="*"
	var readonly := LineEdit.new()
	readonly.editable=false
	readonly.context_menu_enabled=false
	readonly.shortcut_keys_enabled=false
	readonly.hide()
	field.get_parent().add_child(readonly)
	var clear := CheckBox.new()
	clear.text="清除"
	row.add_child(clear)
	var entry := {"field":field,"display":readonly,"stored":false,"clear":clear}
	entry.toggle=_button(row,"顯示",func(): _toggle_secret(key))
	credentials[key]=entry
	clear.toggled.connect(func(_checked: bool): _sync_secret(entry))
	field.text_changed.connect(func(_text: String): _sync_secret(entry))

func _hide_secret(entry: Dictionary) -> void:
	entry.field.secret=true
	entry.field.show()
	entry.display.text=""
	entry.display.hide()
	entry.toggle.text="顯示"

func _hide_credentials() -> void:
	for entry in credentials.values():
		_hide_secret(entry)

func _sync_secret(entry: Dictionary) -> void:
	var cleared: bool=entry.clear.button_pressed
	entry.clear.visible=editing
	entry.field.editable=editing and not cleared
	entry.field.placeholder_text=MASK if entry.stored else "未設定"
	entry.field.tooltip_text="留空保留原值，或輸入新值。" if editing and entry.stored and not cleared else ""
	if cleared:
		_hide_secret(entry)
	entry.toggle.disabled=cleared or (not entry.stored and entry.field.text.is_empty() and entry.field.secret)

func _toggle_secret(key: String) -> void:
	var entry: Dictionary=credentials[key]
	if entry.display.visible or not entry.field.secret:
		_hide_secret(entry)
	elif not entry.field.text.is_empty():
		entry.field.secret=false
		entry.toggle.text="隱藏"
	elif entry.stored:
		var response := _request("ai",{"provider":displayed_source}) if key=="ai_key" else _request("reveal_telegram")
		if not response.ok:
			status.text=response.error
			return
		var value: Variant=response.data.get("key") if key=="ai_key" else response.data
		if not value is String or value.is_empty():
			status.text="目前沒有儲存值。"
			return
		entry.field.hide()
		entry.display.text=value
		entry.display.show()
		entry.toggle.text="隱藏"
	_sync_secret(entry)

func _set_editing(value: bool) -> void:
	editing=value
	edit_button.visible=not editing
	cancel_button.visible=editing
	save_button.visible=editing
	model.editable=editing
	model_presets.disabled=not editing
	model_choices.editable=editing
	provider.disabled=not editing or not key_setup_source.is_empty()
	provider_caption.text="金鑰所屬來源" if not key_setup_source.is_empty() else "預設 AI 來源"
	chat_id.editable=editing
	_hide_credentials()
	for entry in credentials.values():
		_sync_secret(entry)

func _reload() -> void:
	key_setup_source=""
	var response := _request("read")
	if not response.ok:
		status.text=response.error
		_set_editing(false)
		edit_button.disabled=true
		return
	edit_button.disabled=false
	var saved: Dictionary=response.data
	saved_sources=saved.ai_sources
	saved_provider=saved.ai_provider
	source_drafts.clear()
	provider.select(maxi(0,AI.NAMES.keys().find(saved_provider)))
	_show_source(saved_provider)
	chat_id.text=saved.telegram_chat_id
	var telegram: Dictionary=credentials.telegram_bot_token
	telegram.stored=saved.telegram_bot_token_set
	telegram.clear.set_pressed_no_signal(false)
	telegram.field.text=""
	_set_editing(false)
	status.text=""

func open_provider_key(source: String) -> void:
	if edit_button.disabled or not AI.NAMES.has(source): return
	key_setup_source=source
	provider.select(AI.NAMES.keys().find(source))
	_show_source(source)
	_set_editing(true)
	status.text="填寫 %s API 金鑰後，按「儲存設定」。" % AI.NAMES[source]
	credentials.ai_key.field.grab_focus.call_deferred()

func _save() -> void:
	if not editing:
		return
	_capture_source()
	var data := {"ai_provider":displayed_source if key_setup_source.is_empty() else saved_provider,"telegram_chat_id":chat_id.text}
	for source in source_drafts:
		var draft: Dictionary=source_drafts[source]
		if source=="openai":
			data.openai_model=draft.model
			data.openai_models=draft.models
			if draft.has("key"):
				data.openai_key=draft.key
		else:
			data[source]=draft
	var entry: Dictionary=credentials.telegram_bot_token
	var value: String=entry.field.text.strip_edges()
	if entry.clear.button_pressed or not value.is_empty():
		data.telegram_bot_token="" if entry.clear.button_pressed else value
	var response := _request("save",data)
	if not response.ok:
		status.text=response.error
		return
	var configured := key_setup_source
	_reload()
	status.text="已儲存工作台設定。" if configured.is_empty() else "已儲存 %s API 設定。" % AI.NAMES[configured]
	saved.emit()
