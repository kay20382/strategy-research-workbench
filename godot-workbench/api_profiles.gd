extends "res://skin.gd"

const PROVIDERS := ["kgi_quotecom", "kgi_tradecom", ""]
const TRADE_SECRETS := ["trade_account", "sub_account"]
const SAVED_SECRET_MASK := "******"

var bridge: RefCounted
var root := ""
var profile_list: Tree
var filter: OptionButton
var empty_list: Label
var editor_title: Label
var editor_scroll: ScrollContainer
var discard_dialog: ConfirmationDialog
var pending_change: Callable
var selected_profile_id := ""
var saved_form := {}
var purpose: OptionButton
var provider_type: OptionButton
var status: Label
var save_button: Button
var edit_button: Button
var cancel_button: Button
var certificate_button: Button
var extra_toolbar: HBoxContainer
var editing := false
var schema_ok := false
var fields := {}
var secrets := {}
var rows: Array = []
var certificate: HBoxContainer
var extras: VBoxContainer
var extra_rows: Array[Dictionary] = []
var schema_status: Label
var custom_fields := false

func _ready() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	if root.is_empty():
		root = ProjectSettings.globalize_path("res://..").simplify_path()
	var layout := HBoxContainer.new()
	layout.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	layout.add_theme_constant_override("separation", 12)
	add_child(layout)
	_build_list(layout)
	editor_scroll = ScrollContainer.new()
	editor_scroll.size_flags_horizontal = SIZE_EXPAND_FILL
	editor_scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	layout.add_child(editor_scroll)
	var panel := _panel(editor_scroll)
	var editor_header := _row(panel)
	editor_title = _label(editor_header, "新增 API 設定", 20, TEAL)
	editor_title.size_flags_horizontal = SIZE_EXPAND_FILL
	editor_title.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	edit_button = _button(editor_header, "修改設定", func():
		editing = true
		_hide_secrets()
		_sync_editing(), true)
	cancel_button = _button(editor_header, "取消修改", func():
		_confirm_change(func(): _refresh(selected_profile_id)))

	var identity := _row(panel)
	fields.id = _field(identity, "設定名稱")
	fields.id.placeholder_text = "例如 kgi-market"
	var provider_column := VBoxContainer.new()
	provider_column.size_flags_horizontal = SIZE_EXPAND_FILL
	identity.add_child(provider_column)
	_label(provider_column, "券商 API", 14, MUTED)
	provider_type = OptionButton.new()
	provider_type.add_item("凱基 QuoteCom")
	provider_type.add_item("凱基 TradeCom")
	provider_type.add_item("自訂券商 API")
	provider_column.add_child(provider_type)
	fields.provider = _field(provider_column, "供應商代碼")
	fields.provider.placeholder_text = "例如 broker_api"
	fields.provider.text_changed.connect(func(_value: String): _change_schema())
	provider_type.item_selected.connect(_choose_provider)
	var use_column := VBoxContainer.new()
	identity.add_child(use_column)
	_label(use_column, "用途", 14, MUTED)
	purpose = OptionButton.new()
	purpose.add_item("行情")
	purpose.add_item("下單")
	use_column.add_child(purpose)
	purpose.item_selected.connect(func(_index: int): _change_schema())
	schema_status = _label(panel, "", 14, MUTED)

	var address := _row(panel)
	fields.connection = _field(address, "固定連線名稱")
	fields.host = _field(address, "連線 IP 或主機 (Host)")
	fields.port = _field(address, "連接埠 (Port)")
	fields.source_id = _field(address, "來源 (SourceID)")
	_secret_field(_row(panel), "account", "使用者 (UserID)")
	_secret_field(_row(panel), "password", "密碼 (PassWord)")
	_secret_field(_row(panel), "token", "權杖 (Token)")
	var trade := _row(panel)
	fields.broker_id = _field(trade, "券商代碼 (BrokerID)")
	_secret_field(_row(panel), "trade_account", "交易帳號 (Account)")
	_secret_field(_row(panel), "sub_account", "交易子帳 (SubAccount)")

	certificate = _row(panel)
	fields.ca_path = _field(certificate, "CA 憑證路徑")
	var chooser := FileDialog.new()
	chooser.title = "選取 CA 憑證"
	chooser.access = FileDialog.ACCESS_FILESYSTEM
	chooser.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	chooser.use_native_dialog = true
	chooser.file_selected.connect(func(path: String): fields.ca_path.text = path)
	add_child(chooser)
	certificate_button = _button(certificate, "選取憑證", chooser.popup_file_dialog)
	_secret_field(_row(panel), "ca_password", "CA 密碼")
	extras = VBoxContainer.new()
	panel.add_child(extras)
	extra_toolbar = _row(extras)
	_button(extra_toolbar, "新增公開參數", func(): _extra_field(false))
	_button(extra_toolbar, "新增敏感參數", func(): _extra_field(true))

	var footer := _row(panel)
	save_button = _button(footer, "儲存設定", _save, true)
	status = _label(footer, "", 14, MUTED)
	status.size_flags_horizontal = SIZE_EXPAND_FILL
	status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	visibility_changed.connect(func():
		if not is_visible_in_tree():
			_hide_secrets())
	if not ClassDB.class_exists("ApiProfileBridge"):
		status.text = "找不到 API 設定模組。"
		save_button.disabled = true
		return
	bridge = ClassDB.instantiate("ApiProfileBridge")
	_refresh("")
	if not rows.is_empty():
		_select_profile(0)

func _build_list(layout: HBoxContainer) -> void:
	var sidebar := _panel(layout)
	var sidebar_panel := sidebar.get_parent() as PanelContainer
	sidebar_panel.size_flags_horizontal = SIZE_FILL
	sidebar_panel.custom_minimum_size.x = 390
	_label(sidebar, "行情／交易 API", 20, TEAL)
	var tools := _row(sidebar)
	filter = OptionButton.new()
	filter.add_item("全部")
	filter.add_item("行情")
	filter.add_item("交易")
	filter.select(0)
	filter.size_flags_horizontal = SIZE_EXPAND_FILL
	filter.item_selected.connect(func(_index: int): _update_list())
	tools.add_child(filter)
	_button(tools, "重新讀取", func(): _confirm_change(func(): _refresh(selected_profile_id)))
	_button(tools, "新增 API", _new_profile, true)

	profile_list = Tree.new()
	profile_list.columns = 3
	profile_list.hide_root = true
	profile_list.select_mode = Tree.SELECT_ROW
	profile_list.set_column_titles_visible(true)
	profile_list.set_column_title(0, "設定名稱")
	profile_list.set_column_title(1, "券商 API")
	profile_list.set_column_title(2, "用途")
	profile_list.set_column_expand(2, false)
	profile_list.set_column_custom_minimum_width(2, 52)
	profile_list.size_flags_vertical = SIZE_EXPAND_FILL
	profile_list.add_theme_constant_override("v_separation", 10)
	profile_list.add_theme_stylebox_override("panel", _style(Color("0c1627")))
	profile_list.add_theme_stylebox_override("selected", _style(Color("1e4450"), TEAL, 6))
	profile_list.add_theme_stylebox_override("selected_focus", _style(Color("1e4450"), TEAL, 6))
	profile_list.item_selected.connect(_list_selected)
	sidebar.add_child(profile_list)
	empty_list = _label(sidebar, "", 14, MUTED)

	discard_dialog = ConfirmationDialog.new()
	discard_dialog.title = "尚未儲存"
	discard_dialog.dialog_text = "目前的 API 設定有尚未儲存的修改。要放棄修改嗎？"
	discard_dialog.ok_button_text = "放棄修改"
	discard_dialog.cancel_button_text = "繼續編輯"
	discard_dialog.confirmed.connect(func():
		if pending_change.is_valid():
			pending_change.call()
		pending_change = Callable())
	discard_dialog.canceled.connect(func(): pending_change = Callable())
	add_child(discard_dialog)

func _secret_field(parent: Node, key: String, caption: String) -> void:
	var field := _field(parent, caption)
	field.secret = true
	var clear := CheckBox.new()
	clear.text = "清除"
	parent.add_child(clear)
	var entry := _visibility_control(parent, field, key, key in TRADE_SECRETS)
	entry.clear = clear
	entry.row = parent
	clear.toggled.connect(func(_enabled: bool): _sync_visibility(entry))
	secrets[key] = entry

func _visibility_control(parent: Node, field: LineEdit, key: String, custom: bool) -> Dictionary:
	field.secret_character = "*"
	var display := LineEdit.new()
	display.editable = false
	display.context_menu_enabled = false
	display.shortcut_keys_enabled = false
	display.hide()
	field.get_parent().add_child(display)
	var entry := {"field":field, "display":display, "key":key, "custom":custom, "stored":false, "clear":null}
	entry.toggle = _button(parent, "顯示", func(): _toggle_secret(entry))
	field.text_changed.connect(func(_value: String): _sync_visibility(entry))
	return entry

func _hide_secret(entry: Dictionary) -> void:
	entry.display.text = ""
	entry.display.hide()
	entry.field.show()
	entry.field.secret = true
	entry.toggle.text = "顯示"

func _hide_secrets() -> void:
	for entry in secrets.values():
		_hide_secret(entry)
	for entry in extra_rows:
		if entry.sensitive:
			_hide_secret(entry.visibility)

func _sync_visibility(entry: Dictionary) -> void:
	var cleared: bool = entry.clear != null and entry.clear.button_pressed
	entry.field.editable = editing and not cleared
	entry.field.placeholder_text = SAVED_SECRET_MASK if entry.stored else "未設定"
	entry.field.tooltip_text = "留空保留原值，或輸入新值。" if editing and entry.stored and not cleared else ""
	if entry.clear != null:
		entry.clear.disabled = not editing
	if cleared:
		_hide_secret(entry)
	entry.toggle.disabled = cleared or (entry.field.text.is_empty() and not entry.stored and entry.field.secret)

func _toggle_secret(entry: Dictionary) -> void:
	if entry.display.visible or not entry.field.secret:
		_hide_secret(entry)
	elif not entry.field.text.is_empty():
		entry.field.secret = false
		entry.toggle.text = "隱藏"
	elif entry.stored and bridge != null:
		var response: Variant = JSON.parse_string(bridge.reveal(root, selected_profile_id, entry.key, entry.custom))
		if response is Dictionary and response.get("ok", false):
			entry.field.hide()
			entry.display.text = response.data
			entry.display.show()
			entry.toggle.text = "隱藏"
		else:
			status.text = response.get("error", "無法讀取已儲存值。") if response is Dictionary else "無法讀取已儲存值。"
	_sync_visibility(entry)

func _sync_editing() -> void:
	var existing := not selected_profile_id.is_empty()
	edit_button.visible = existing and not editing
	cancel_button.visible = existing and editing
	save_button.visible = editing
	save_button.disabled = not editing or not schema_ok
	provider_type.disabled = not editing
	purpose.disabled = not editing or provider_type.selected < 2
	certificate_button.disabled = not editing
	extra_toolbar.visible = editing
	for key in fields:
		fields[key].editable = editing and (key != "id" or not existing)
	for entry in secrets.values():
		entry.clear.visible = editing
		_sync_visibility(entry)
	for entry in extra_rows:
		entry.name_field.editable = editing and entry.clear == null
		if entry.sensitive:
			_sync_visibility(entry.visibility)
		else:
			entry.value.editable = editing
		if entry.clear != null:
			entry.clear.visible = editing
		if entry.has("remove"):
			entry.remove.visible = editing

func _choose_provider(index: int) -> void:
	fields.provider.text = PROVIDERS[index]
	if index < 2:
		purpose.select(index)
	_change_schema()

func _change_schema() -> void:
	_hide_secrets()
	_reset_extras()
	for key in ["connection", "host", "port", "source_id", "ca_path", "broker_id"]:
		fields[key].text = ""
	for key in secrets:
		secrets[key].stored = false
		secrets[key].field.text = ""
		secrets[key].clear.button_pressed = false
	_sync_schema()
	_sync_editing()

func _sync_schema() -> void:
	fields.provider.get_parent().visible = provider_type.selected == 2
	var response := _request("api_profile_schema", {
		"provider": fields.provider.text.strip_edges(),
		"purpose": ["quote", "trade"][purpose.selected],
	})
	custom_fields = response.ok and response.data.supports_custom_fields
	extras.visible = custom_fields
	schema_ok = response.ok
	if response.ok:
		var definitions := {}
		for definition in response.data.fields:
			definitions[definition.key] = definition
		for key in fields:
			if key == "id" or key == "provider":
				continue
			var column: VBoxContainer = fields[key].get_parent()
			column.visible = definitions.has(key)
			if column.visible:
				var caption := column.get_child(0) as Label
				caption.text = definitions[key].label
		for key in secrets:
			secrets[key].row.visible = definitions.has(key)
			if secrets[key].row.visible:
				var caption: Label = secrets[key].field.get_parent().get_child(0)
				caption.text = definitions[key].label
		certificate.visible = definitions.has("ca_path")
		schema_status.text = "" if response.data.adapter_available else "目前可儲存設定。連線需接入對應轉接器。"
	else:
		schema_status.text = response.error

func _reset_extras() -> void:
	for entry in extra_rows:
		extras.remove_child(entry.row)
		entry.row.queue_free()
	extra_rows.clear()

func _extra_field(sensitive: bool, key: String = "", value: String = "", stored: bool = false) -> void:
	var row := _row(extras)
	var name_field := _field(row, "參數名稱", key)
	var value_field := _field(row, "敏感值" if sensitive else "公開值", value)
	value_field.secret = sensitive
	if stored:
		name_field.editable = false
	var entry := {"row": row, "name_field": name_field, "value": value_field, "sensitive": sensitive, "clear": null}
	extra_rows.append(entry)
	if sensitive:
		entry.visibility = _visibility_control(row, value_field, key, true)
		entry.visibility.stored = stored
	if stored:
		var clear := CheckBox.new()
		clear.text = "清除"
		clear.toggled.connect(func(_enabled: bool): _sync_visibility(entry.visibility))
		row.add_child(clear)
		entry.clear = clear
		entry.visibility.clear = clear
	else:
		entry.remove = _button(row, "移除", func(): extra_rows.erase(entry); row.queue_free())
	_sync_editing()

func _request(action: String, payload: Dictionary) -> Dictionary:
	if bridge == null:
		return {"ok": false, "error": "找不到 API 設定模組。"}
	var response: Variant = JSON.parse_string(bridge.request(root, action, JSON.stringify(payload)))
	return response if response is Dictionary else {"ok": false, "error": "無法讀取設定回應。"}

func _refresh(id: String) -> void:
	var response := _request("api_profile_list", {})
	if not response.ok:
		status.text = response.error
		return
	rows = response.data.profiles
	var selected := -1
	for i in rows.size():
		if rows[i].profile.id == id:
			selected = i
			break
	_select_profile(selected)

func _update_list() -> void:
	profile_list.set_block_signals(true)
	profile_list.clear()
	var list_root := profile_list.create_item()
	var requested_purpose: String = ["", "quote", "trade"][filter.selected]
	for i in rows.size():
		var profile: Dictionary = rows[i].profile
		if not requested_purpose.is_empty() and profile.purpose != requested_purpose:
			continue
		var item := profile_list.create_item(list_root)
		var provider_name: String = {"kgi_quotecom":"凱基 QuoteCom", "kgi_tradecom":"凱基 TradeCom"}.get(profile.provider, profile.provider)
		var values: Array[String] = [profile.id, provider_name, "交易" if profile.purpose == "trade" else "行情"]
		for column in values.size():
			item.set_text(column, values[column])
			item.set_tooltip_text(column, values[column])
			item.set_text_overrun_behavior(column, TextServer.OVERRUN_TRIM_ELLIPSIS)
		item.set_metadata(0, i)
		if profile.id == selected_profile_id:
			item.select(0)
	profile_list.set_block_signals(false)
	empty_list.visible = list_root.get_child_count() == 0
	empty_list.text = "尚無 API 設定" if rows.is_empty() else "這個類別尚無 API 設定"

func _list_selected() -> void:
	var item := profile_list.get_selected()
	if item == null:
		return
	var index: int = item.get_metadata(0)
	if rows[index].profile.id != selected_profile_id:
		_confirm_change.call_deferred(func(): _select_profile(index))

func _new_profile() -> void:
	_confirm_change(func():
		_select_profile(-1)
		fields.id.grab_focus())

func open_tradecom() -> void:
	_confirm_change(func():
		filter.select(2)
		for i in rows.size():
			if rows[i].profile.provider=="kgi_tradecom" and rows[i].profile.purpose=="trade":
				_select_profile(i)
				return
		_select_profile(-1)
		fields.id.grab_focus())

func _form_state() -> Dictionary:
	var state := {"purpose": purpose.selected, "provider_type": provider_type.selected, "extras": []}
	for key in fields:
		state[key] = fields[key].text
	for key in secrets:
		state[key] = [not secrets[key].field.text.is_empty(), secrets[key].clear.button_pressed]
	for entry in extra_rows:
		var value: Variant = not entry.value.text.is_empty() if entry.sensitive else entry.value.text
		state.extras.append([entry.name_field.text, value, entry.sensitive, entry.clear != null and entry.clear.button_pressed])
	return state

func _confirm_change(callback: Callable) -> void:
	if saved_form.is_empty() or saved_form == _form_state():
		callback.call()
		return
	_update_list()
	pending_change = callback
	discard_dialog.popup_centered()

func _select_profile(index: int) -> void:
	_hide_secrets()
	var entry: Dictionary = rows[index] if index >= 0 else {}
	var profile: Dictionary = entry.get("profile", {})
	selected_profile_id = profile.get("id", "")
	editing = index < 0
	var default_purpose := "trade" if filter.selected == 2 else "quote"
	var default_provider := "kgi_tradecom" if default_purpose == "trade" else "kgi_quotecom"
	for key in fields:
		var value: Variant = profile.get(key, default_provider if key == "provider" else "")
		fields[key].text = str(value) if value != null else ""
	if profile.get("port") != null:
		fields.port.text = str(int(profile.port))
	var is_tradecom: bool = fields.provider.text == "kgi_tradecom" and profile.get("purpose", default_purpose) == "trade"
	if is_tradecom:
		fields.broker_id.text = profile.get("options", {}).get("broker_id", "")
	purpose.select(1 if profile.get("purpose", default_purpose) == "trade" else 0)
	var provider_index := PROVIDERS.find(fields.provider.text)
	provider_type.select(provider_index if provider_index >= 0 else 2)
	_reset_extras()
	for key in profile.get("options", {}):
		if not is_tradecom:
			_extra_field(false, key, profile.options[key])
	for key in entry.get("secret_keys", []):
		if not is_tradecom:
			_extra_field(true, key, "", true)
	for key in secrets:
		var field: LineEdit = secrets[key].field
		secrets[key].clear.button_pressed = false
		field.text = ""
		var is_set: bool = key in entry.get("secret_keys", []) if key in TRADE_SECRETS else entry.get(key + "_set", false)
		secrets[key].stored = is_set
	status.text = "" if entry.get("credentials_readable", true) else "目前的 Windows 帳戶無法讀取此設定的憑證。"
	_sync_schema()
	_sync_editing()
	editor_title.text = "新增 API 設定" if index < 0 else selected_profile_id
	editor_title.tooltip_text = editor_title.text
	editor_scroll.scroll_vertical = 0
	saved_form = _form_state()
	_update_list()

func _save() -> void:
	if not editing or not schema_ok:
		return
	var profile := {}
	for key in fields:
		profile[key] = fields[key].text.strip_edges()
	var broker_id: String = profile.get("broker_id", "")
	profile.erase("broker_id")
	var selected := {}
	for row in rows:
		if row.profile.id.nocasecmp_to(profile.id) == 0:
			if selected_profile_id.is_empty():
				status.text = "已有同名設定，請從清單選取後編輯。"
				return
			selected = row.profile
			break
	var port: String = profile.port
	if not port.is_empty() and (not port.is_valid_int() or int(port) < 1 or int(port) > 65535):
		status.text = "連接埠須為 1 至 65535。"
		return
	profile.port = null if port.is_empty() else int(port)
	profile.purpose = ["quote", "trade"][purpose.selected]
	var is_tradecom: bool = profile.provider == "kgi_tradecom" and profile.purpose == "trade"
	if not custom_fields:
		var same_profile: bool = selected.get("provider") == profile.provider and selected.get("purpose") == profile.purpose
		profile.connection = selected.get("connection", "") if same_profile else ""
		profile.ca_path = selected.get("ca_path", "") if same_profile else ""
	var credentials := {}
	for key in secrets:
		if key in TRADE_SECRETS or not secrets[key].row.visible:
			continue
		var value: String = secrets[key].field.text
		if secrets[key].clear.button_pressed:
			credentials[key] = ""
		elif not value.is_empty():
			credentials[key] = value
	if is_tradecom:
		profile.options = selected.get("options", {}).duplicate() if selected.get("provider") == profile.provider and selected.get("purpose") == profile.purpose else {}
		if broker_id.is_empty():
			profile.options.erase("broker_id")
		else:
			profile.options.broker_id = broker_id
		credentials.secrets = {}
		for key in TRADE_SECRETS:
			var value: String = secrets[key].field.text.strip_edges()
			if secrets[key].clear.button_pressed:
				credentials.secrets[key] = ""
			elif not value.is_empty():
				credentials.secrets[key] = value
	elif custom_fields:
		profile.options = {}
		credentials.secrets = {}
		var seen := {}
		for entry in extra_rows:
			var key: String = entry.name_field.text.strip_edges()
			if key.is_empty() or seen.has(key):
				status.text = "自訂參數須填寫名稱，且不可同名。"
				return
			seen[key] = true
			var value: String = entry.value.text
			if not entry.sensitive:
				profile.options[key] = value
			elif entry.clear != null and entry.clear.button_pressed:
				credentials.secrets[key] = ""
			elif not value.is_empty():
				credentials.secrets[key] = value
	var response := _request("api_profile_save", {"profile": profile, "credentials": credentials})
	if not response.ok:
		status.text = response.error
		return
	if filter.selected != 0 and filter.selected != purpose.selected + 1:
		filter.select(0)
	_refresh(response.data.profile.id)
	status.text = "已儲存 API 設定。"
