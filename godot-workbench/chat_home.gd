extends "res://skin.gd"

const ACTIONS := ["reply", "market", "sources", "coverage", "market_import", "market_export",
	"code", "open", "new", "import", "save", "check", "backtest", "run", "report", "optimize", "api", "tradecom", "draft", "settings", "strategy_catalog", "clarify", "research_plan"]
const RESEARCH_ACTIONS := ["draft","run","optimize"]
const GRILL_ME := "依 grill-me 流程確認策略驗收。先從已知策略與回測設定取用事實。若有會改變結果的歧義，使用 clarify 提出本輪可回答的問題，等回覆後再問有相依性的問題。需求完整時使用 research_plan，research_action 填 draft、run 或 optimize，message 列出具體可查核的策略、商品與週期、規則與例外、回測期間與成本、門檻及證據中適用的項目。每筆平倉須 exit_bar > entry_bar。第一階段禁止 draft、run、optimize 或輸出原碼。只有收到工作台的已確認驗收項目後，才執行獲確認的 research_action。"
const REVIEW_ROUTING := "策略需求的題目及選項由策略研究 MCP 產生。從零新建策略必須進入驗收流程，review_mode 填 new，逐項確認商品、K 棒週期、交易時段及行情來源、策略目標、預期進場、出場、停損與停利。改既有策略填 existing。模糊需求使用 clarify，research_action 填預定操作。acceptance 只填入使用者已明確表達或選定的條件，未知留空。確認清單前不得在 message 或其他欄位自行撰寫提問、建議答案或選項。"
const DT = preload("res://date_format.gd")
const AI = preload("res://ai_api.gd")
const TELEGRAM_ACTIONS := ["reply","sources","coverage","open","check","run","report","optimize","strategy_catalog"]
const STARTERS := ["查詢 TXF1 行情來源", "檢查目前策略語法", "開始回測", "摘要最近回測報表", "開啟 API 設定"]

var studio: Control
var composer: TextEdit
var transcript: VBoxContainer
var welcome: VBoxContainer
var top_space: Control
var bottom_space: Control
var scroll: ScrollContainer
var margins: MarginContainer
var send: Button
var status: Label
var context: Label
var request: HTTPRequest
var pending := ""
var after_open := {}
var draft_source := ""
var market_symbol := ""
var market_action := ""
var model_document_id := ""
var model_picker: OptionButton
var request_provider := "openai"
var telegram_reply: Callable
var research_review := {}
var review_form: VBoxContainer
var approved_review := {}
var model_prompt := ""
var model_context := {}
var requirements_bridge: RefCounted
var requirements_intent := {}
var requirements_acceptance := {}

func _ready() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margins = MarginContainer.new()
	margins.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	for side in ["top", "bottom"]:
		margins.add_theme_constant_override("margin_" + side, 24)
	add_child(margins)
	resized.connect(_resize_margins)
	_resize_margins()
	var layout := VBoxContainer.new()
	layout.add_theme_constant_override("separation", 16)
	margins.add_child(layout)

	top_space = Control.new()
	top_space.size_flags_vertical = SIZE_EXPAND_FILL
	layout.add_child(top_space)
	welcome = VBoxContainer.new()
	welcome.add_theme_constant_override("separation", 14)
	layout.add_child(welcome)
	scroll = ScrollContainer.new()
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	scroll.size_flags_vertical = SIZE_EXPAND_FILL
	scroll.hide()
	layout.add_child(scroll)
	transcript = VBoxContainer.new()
	transcript.size_flags_horizontal = SIZE_EXPAND_FILL
	transcript.add_theme_constant_override("separation", 14)
	scroll.add_child(transcript)
	var title := _label(welcome, "今天想研究什麼？", 36, INK)
	title.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	title.custom_minimum_size.y = 90
	var intro := _label(welcome, "查詢行情、開啟策略、執行回測或讀取報表。", 17, MUTED)
	intro.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	var suggestions := HFlowContainer.new()
	suggestions.alignment = FlowContainer.ALIGNMENT_CENTER
	welcome.add_child(suggestions)
	for prompt in STARTERS:
		_button(suggestions, prompt, func():
			composer.text = prompt
			composer.grab_focus())

	var panel := _panel(layout)
	context = _label(panel, "正在讀取策略庫…", 14, TEAL)
	context.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	composer = TextEdit.new()
	composer.placeholder_text = "輸入提示詞，例如「回測 _kyleTest1」"
	composer.custom_minimum_size.y = 112
	composer.wrap_mode = TextEdit.LINE_WRAPPING_BOUNDARY
	composer.add_theme_stylebox_override("normal", _style(Color("0b1423")))
	composer.add_theme_stylebox_override("focus", _style(Color("0b1423"), TEAL))
	panel.add_child(composer)
	composer.gui_input.connect(_composer_input)
	composer.text_changed.connect(_sync_send)
	var controls := _row(panel)
	model_picker=OptionButton.new()
	model_picker.fit_to_longest_item=false
	model_picker.custom_minimum_size.x=248
	model_picker.tooltip_text="需要 AI 理解時，自動使用所選模型。詢問報表時會附上目前策略的績效摘要。"
	controls.add_child(model_picker)
	refresh_models()
	status = _label(controls, "Ctrl + Enter 送出", 14, MUTED)
	status.size_flags_horizontal = SIZE_EXPAND_FILL
	status.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	status.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS
	send = _button(controls, "送出", _submit, true)
	send.tooltip_text="Ctrl + Enter 送出"
	model_picker.item_selected.connect(func(_index): _sync_send())
	bottom_space = Control.new()
	bottom_space.size_flags_vertical = SIZE_EXPAND_FILL
	bottom_space.size_flags_stretch_ratio = 1.2
	layout.add_child(bottom_space)
	request = HTTPRequest.new()
	request.timeout = 90
	request.max_redirects = 0
	request.body_size_limit = 1_000_000
	request.request_completed.connect(_model_done)
	add_child(request)
	_sync_send()

func _resize_margins() -> void:
	var inset := maxi(24, int((size.x - 1040) / 2.0))
	for side in ["left", "right"]:
		margins.add_theme_constant_override("margin_" + side, inset)

func refresh_models() -> void:
	var choice := _model_selection()
	var settings: Control=studio.settings
	var default_source: String=settings.saved_provider
	model_picker.clear()
	var default_profile: Dictionary=settings.saved_sources.get(default_source,{})
	var default_model: String=default_profile.get("model","")
	if default_model.is_empty():
		default_model=studio.workbench.optimizer.model.text.strip_edges() if default_source=="openai" else AI.MODELS[default_source][0]
	model_picker.add_item("預設 · "+AI.NAMES[default_source]+" · "+default_model)
	model_picker.set_item_metadata(0,{})
	var selected := 0
	for source in AI.NAMES:
		var profile: Dictionary=settings.saved_sources.get(source,{})
		var models: Array=[profile.get("model","")]
		models.append_array(profile.get("models",[]))
		for name in AI.available_models(source,models):
			var index := model_picker.item_count
			var metadata := {"provider":source,"model":name}
			model_picker.add_item(AI.NAMES[source]+" · "+name)
			model_picker.set_item_metadata(index,metadata)
			if metadata==choice:
				selected=index
	model_picker.select(selected)
	if send!=null: _sync_send()

func _model_selection() -> Dictionary:
	return model_picker.get_item_metadata(model_picker.selected) if model_picker.selected>=0 else {}

func _process(_delta: float) -> void:
	if pending=="requirements": _poll_requirements()
	var wb: Control = studio.workbench
	var text := "尚未開啟策略"
	if wb.active >= 0:
		text = "目前策略 · " + str(wb.documents[wb.active].name).trim_prefix("REA_")
	if context.text != text:
		context.text = text

func _composer_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo and event.ctrl_pressed and event.keycode in [KEY_ENTER, KEY_KP_ENTER]:
		composer.accept_event()
		_submit()

func _sync_send() -> void:
	send.disabled = not pending.is_empty() or composer.text.strip_edges().is_empty()
	model_picker.disabled=not pending.is_empty()
	var settings: Control=studio.settings
	var provider: String=_model_selection().get("provider",settings.saved_provider)
	var saved: Dictionary=settings.saved_sources.get(provider,{})
	var key_set: bool=saved.get("key_set",false) or not OS.get_environment(AI.ENV_KEYS[provider]).strip_edges().is_empty()
	if provider=="openai" and not studio.workbench.optimizer.api_key.text.strip_edges().is_empty():
		key_set=true
	status.text="金鑰已設定" if key_set else "尚未設定金鑰"
	if not pending.is_empty():
		status.text="正在等待 AI 回覆…" if pending=="model" else "正在處理…"
	status.tooltip_text=AI.NAMES[provider]+" · "+status.text+"\nCtrl + Enter 送出"

func _bubble(user: bool, text: String, tab: int = -1, label: String = "", report := false, remote := false) -> void:
	welcome.hide()
	top_space.hide()
	bottom_space.hide()
	scroll.show()
	var card := _panel(transcript)
	var box := card.get_parent() as PanelContainer
	box.add_theme_stylebox_override("panel", _style(Color("15263e") if user else Color("101b2d")))
	_label(card, ("Telegram" if remote else "你") if user else "工作台", 14, BLUE if user else TEAL)
	var body := _label(card, text, 16)
	body.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	body.custom_minimum_size.x = 240
	if tab >= 0:
		_button(card, label, func():
			if tab==studio.SETTINGS: studio.open_settings(studio.SETTINGS_AI)
			else: studio.open_page(tab)
			if report:
				studio.workbench.pages.current_tab = 2)
	var bar := scroll.get_v_scroll_bar()
	bar.set_deferred("value", bar.max_value)
	scroll.set_deferred("scroll_vertical", 100000)

func _finish(text: String, tab: int = -1, label: String = "", report := false) -> void:
	pending = ""
	approved_review.clear()
	after_open = {}
	draft_source = ""
	_bubble(false, text, tab, label, report)
	_sync_send()
	var reply := telegram_reply
	telegram_reply=Callable()
	if reply.is_valid():
		reply.call(text)

func _submit() -> void:
	var text := composer.text.strip_edges()
	if not pending.is_empty() or text.is_empty():
		return
	if text.length() > 12000:
		status.text = "每則提示詞最多 12,000 字。"
		return
	composer.clear()
	_submit_text(text)

func submit_telegram(text: String, reply: Callable) -> void:
	if not pending.is_empty() or telegram_reply.is_valid():
		reply.call("工作台正在處理另一項作業，請等完成後再送出。")
		return
	if text.length()>4000:
		reply.call("請將提示詞縮短至 4,000 字以內。")
		return
	telegram_reply=reply
	_submit_text(text)

func _submit_text(text: String) -> void:
	_bubble(true,text,-1,"",false,telegram_reply.is_valid())
	if not research_review.is_empty() and research_review.remote==telegram_reply.is_valid():
		if is_instance_valid(review_form): review_form.hide()
		if text in ["取消","取消驗收"]:
			research_review.clear()
			_finish("已取消策略需求確認。")
			return
		if text=="確認驗收":
			_confirm_research()
			return
		if research_review.context!=_research_context():
			research_review.clear()
			_finish("策略或回測設定已變更，請重新送出原需求。")
			return
		research_review.action=""
		_ask_model(text)
		return
	var intent := _local_intent(text)
	if intent.get("action")=="optimize":
		_ask_model(text)
	elif not intent.is_empty():
		_execute(intent)
	elif not _offer_backtest_strategies(text):
		_ask_model(text)

func review_optimization() -> void:
	if not pending.is_empty(): return
	var optimizer: Control=studio.workbench.optimizer
	if optimizer.goal.text.strip_edges().is_empty():
		optimizer.detail.text="請先填寫優化目標。"
		return
	studio.open_page(studio.HOME)
	_submit_text("請優化目前策略，執行 %d 輪，排名依據為「%s」。修改目標為：%s" % [
		int(optimizer.rounds.value),optimizer.objective.get_item_text(optimizer.objective.selected),optimizer.goal.text])

func _research_context() -> Dictionary:
	var wb: Control=studio.workbench
	if wb.active<0: return {}
	var doc: Dictionary=wb.documents[wb.active]
	var task: Dictionary=doc.task
	var displayed_settings := {}
	for key in wb.fields:
		displayed_settings[key]=wb.fields[key].text
	return {"id":doc.id,"name":doc.name,"market":task.get("market",{}),
		"parameters":task.get("parameters",{}),"source_hash":wb.editor.text.sha256_text(),
		"displayed_settings":displayed_settings,"input_text":wb.inputs.text,
		"period_unit":wb.unit.selected,"pyramiding":wb.pyramid.selected,
		"summary":doc.get("result").get("summary",{}) if doc.get("result") is Dictionary else {}}

func _confirm_research() -> void:
	if _approve_research():
		_ask_model("依已確認的驗收項目執行。")

func _approve_research() -> bool:
	if research_review.get("action","").is_empty():
		_finish("請先回答尚待確認的問題，工作台會再整理驗收項目。")
		return false
	if research_review.context!=_research_context():
		research_review.clear()
		_finish("策略或回測設定已變更，請重新送出原需求。")
		return false
	approved_review=research_review.duplicate(true)
	research_review.clear()
	return true

func _local_intent(text: String) -> Dictionary:
	if "不要" in text or "先別" in text or "暫不" in text:
		return {}
	if "策略" in text:
		var query := RegEx.new()
		var quantity := "(?:幾[個支隻套種]|多少[個支隻套種]?|哪些)"
		var total := "(?:(?:總共|一共|合計)\\s*)?"
		query.compile("^(?:請問\\s*)?(?:目前\\s*)?"+total+"(?:(?:有"+quantity+"|列出(?:所有)?)\\s*(.*?)策略|(.*?)策略"+total+"(?:有"+quantity+"|清單))[？?。！!\\s]*$")
		var match_query := query.search(text)
		if match_query:
			return {"action":"strategy_catalog","strategy":match_query.get_string(1) + match_query.get_string(2)}
	var strategy := _match_strategy(text)
	var action := ""
	if "tradecom" in text.to_lower():
		action = "tradecom"
	elif text in ["設定","開啟設定","開啟工作台設定"] or ("設定" in text and ("telegram" in text.to_lower() or "openai" in text.to_lower() or "ai api" in text.to_lower())):
		action = "settings"
	elif "api" in text.to_lower() and ("設定" in text or "開啟" in text):
		action = "api"
	elif "行情" in text and "完整性" in text:
		action = "coverage"
	elif "行情" in text and ("來源" in text or "查詢" in text or "資料有" in text):
		action = "sources"
	elif "行情" in text and "匯入" in text:
		action = "market_import"
	elif "行情" in text and "匯出" in text:
		action = "market_export"
	elif "行情" in text and ("開啟" in text or text == "行情資料庫"):
		action = "market"
	elif "匯入" in text and ("策略" in text or "pl" in text.to_lower()):
		action = "import"
	elif text in ["新增策略", "新建策略", "新增空白策略", "建立空白策略"]:
		action = "new"
	elif text == "儲存策略副本":
		action = "save"
	elif "優化" in text and (text.begins_with("優化") or text.begins_with("開始優化") or text.begins_with("開始 AI 優化") or text.begins_with("用 AI 優化")):
		action = "optimize"
	elif "優化" in text and ("開啟" in text or text == "AI 優化"):
		action = "optimizer"
	elif "報表" in text and ("摘要" in text or "最近" in text or "開啟" in text):
		action = "report"
	elif "語法" in text and ("檢查" in text or "驗證" in text):
		action = "check"
	elif text in ["開始回測", "回測目前策略", "執行回測", "重新回測"]:
		action = "run"
		strategy = ""
	elif "回測設定" in text:
		action = "backtest"
	elif not strategy.is_empty() and text.begins_with("開啟"):
		action = "open"
	elif ("程式碼" in text or "策略編輯器" in text) and ("開啟" in text or "切換" in text):
		action = "code"
	if action.is_empty():
		return {}
	var words := RegEx.new()
	words.compile("[A-Za-z][A-Za-z0-9_]{1,19}")
	var symbol := words.search(text) if action in ["sources","coverage"] else null
	var separator := "：" if "：" in text else (":" if ":" in text else "，")
	var goal := text.split(separator, true, 1)
	var rounds_pattern := RegEx.new()
	rounds_pattern.compile("([0-9]+)\\s*輪")
	var rounds := rounds_pattern.search(text) if action == "optimize" else null
	var objective := "drawdown" if "回撤" in text else ("profit" if "淨利" in text else "")
	return {"action":action,"strategy":strategy,"symbol":symbol.get_string() if symbol else "",
		"goal":goal[1].strip_edges() if goal.size() > 1 else "",
		"section":"telegram" if action=="settings" and "telegram" in text.to_lower() else "",
		"source":"","message":"","rounds":int(rounds.get_string(1)) if rounds else 0,"objective":objective}

func _offer_backtest_strategies(text: String) -> bool:
	if "不要" in text or "先別" in text or "暫不" in text or "？" in text or "?" in text:
		return false
	var command := text.trim_prefix("請幫我").trim_prefix("請").strip_edges()
	if not command.begins_with("回測") and not command.begins_with("重新回測"):
		return false
	var target := command.trim_prefix("重新").trim_prefix("回測").strip_edges()
	if command in ["重新回測", "回測目前策略"]:
		_execute({"action":"run","strategy":""})
		return true
	if target.is_empty():
		_finish("請輸入要回測的策略名稱，或輸入「回測目前策略」。")
		return true
	var wb: Control = studio.workbench
	if wb.busy and wb.catalog.is_empty():
		_finish("策略庫正在載入，請稍後再送出。")
		return true
	var matches := _find_strategies(target)
	var exact: Array[Dictionary] = matches.exact
	if exact.size() == 1:
		_execute({"action":"run","strategy":exact[0].id})
		return true
	var candidates: Array[Dictionary] = matches.entries if exact.is_empty() else exact
	if candidates.is_empty():
		_finish("策略庫找不到「%s」。請選擇策略，或輸入策略的完整名稱。" % target, studio.CODE, "開啟策略庫")
		return true
	var names := PackedStringArray()
	for entry in candidates:
		names.append(entry.name.trim_prefix("REA_"))
	if telegram_reply.is_valid():
		var entries := PackedStringArray()
		for index in range(candidates.size()):
			entries.append("%s（%s）" % [names[index], candidates[index].id])
		_finish("請指定要回測的策略名稱或代碼。\n%s" % "\n".join(entries))
		return true
	_finish("找到與「%s」相符的策略。請選擇要回測的版本。選擇後將沿用該版本已儲存的商品、週期、期間與成本，並逐筆檢查平倉棒晚於進場棒。" % target)
	var card: VBoxContainer = transcript.get_child(-1).get_child(0)
	for index in range(candidates.size()):
		var strategy_id: String = candidates[index].id
		var label := "回測 " + names[index]
		if names.count(names[index]) > 1:
			label += "（%s）" % strategy_id
		_button(card, label, func():
			if not pending.is_empty():
				return
			_bubble(true, label)
			_execute({"action":"run","strategy":strategy_id}))
	return true

func _find_strategies(target: String) -> Dictionary:
	var query := target.strip_edges().to_lower().replace(" ", "")
	var keyword := query.trim_suffix("策略")
	var keywords := ["日經", "225m"] if keyword in ["日經", "日經225", "225m"] else [keyword]
	var exact: Array[Dictionary] = []
	var matches: Array[Dictionary] = []
	for entry: Dictionary in studio.workbench.catalog:
		if entry.kind != "strategy":
			continue
		var name: String = entry.name.trim_prefix("REA_").to_lower().replace(" ", "")
		var id: String = entry.id.to_lower()
		if name == query or id == query:
			exact.append(entry)
			matches.append(entry)
		elif query.is_empty() or keywords.any(func(value: String): return not value.is_empty() and (value in name or value in id)):
			matches.append(entry)
	return {"entries":matches,"exact":exact}

func _show_strategy_catalog(target: String) -> void:
	var wb: Control = studio.workbench
	if wb.busy and wb.catalog.is_empty():
		_finish("策略庫正在載入，請稍後再送出。")
		return
	var entries: Array[Dictionary] = _find_strategies(target).entries
	var title := "策略庫共有 %d 個策略版本。" % entries.size() if target.is_empty() else "策略庫中有 %d 個與「%s」相符的策略版本。" % [entries.size(), target]
	var lines := PackedStringArray([title])
	for entry in entries.slice(0, 20):
		lines.append("• " + entry.name.trim_prefix("REA_"))
	if entries.size() > 20:
		lines.append("以上列出前 20 個版本。")
	_finish("\n".join(lines), studio.CODE if not telegram_reply.is_valid() else -1, "開啟策略庫")

func _match_strategy(text: String) -> String:
	var matched := ""
	var lower := text.to_lower()
	for entry in studio.workbench.catalog:
		for candidate: String in [entry.id, entry.name.trim_prefix("REA_")]:
			if candidate.length() > matched.length() and lower.contains(candidate.to_lower()):
				matched = candidate
	return matched

func _show_tab(tab: int, text: String) -> void:
	if telegram_reply.is_valid():
		_finish("已選取策略。" if tab==studio.CODE else text)
		return
	_finish(text)
	studio.open_page(tab)

func _execute(intent: Dictionary) -> void:
	var action: String = intent.get("action", "")
	var wb: Control = studio.workbench
	if telegram_reply.is_valid() and action not in TELEGRAM_ACTIONS:
		_finish("這項操作請在本機工作台進行。Telegram 可查行情、選策略、檢查語法、回測、讀報表和執行 AI 優化。")
		return
	match action:
		"reply":
			_finish(str(intent.get("message","")).left(8000))
			return
		"strategy_catalog":
			_show_strategy_catalog(str(intent.get("strategy","")).strip_edges())
			return
		"market":
			_show_tab(studio.MARKET, "已開啟行情資料庫。")
			return
		"settings":
			_finish("已開啟設定。")
			studio.open_settings(studio.SETTINGS_TELEGRAM if intent.get("section","")=="telegram" else studio.SETTINGS_AI)
			return
		"market_import", "market_export":
			_show_tab(studio.MARKET, "請先選取商品與資料種類，再使用行情管理頁選取檔案。")
			return
		"api", "tradecom":
			_finish("已開啟行情與交易 API 設定。")
			studio.open_settings(studio.SETTINGS_BROKER,action=="tradecom")
			return
		"optimizer":
			_show_tab(studio.OPTIMIZE, "已開啟 AI 優化。")
			return
		"sources", "coverage":
			_start_market(action, str(intent.get("symbol", "")))
			return
	if wb.busy or wb.optimizer.running or wb.active < 0:
		_finish("策略工作台正在處理其他作業，請稍後再送出。")
		return
	var target: String = str(intent.get("strategy", "")).strip_edges()
	if action in ["open","check","run","report","optimize"] and not target.is_empty():
		var matches: Array = wb.catalog.filter(func(entry: Dictionary): return target.nocasecmp_to(entry.id)==0)
		if matches.is_empty():
			matches = wb.catalog.filter(func(entry: Dictionary): return target.nocasecmp_to(entry.name.trim_prefix("REA_"))==0)
		if matches.size() != 1:
			_finish("找不到唯一的「%s」，請到策略庫選取正確版本。" % target, studio.CODE, "開啟策略庫")
			return
		if wb.documents[wb.active].get("managed_id") != matches[0].id and wb.documents[wb.active].get("origin") != matches[0].source:
			if not wb._capture():
				_finish("目前策略設定需要修正。%s" % wb.status.text, studio.CODE, "查看目前策略")
				return
			after_open = intent.duplicate()
			after_open.strategy = ""
			_start_workbench("open", {"entry":matches[0]})
			return
		intent.strategy = ""
	match action:
		"code", "open":
			_show_tab(studio.CODE, "已開啟程式碼工作區。")
		"backtest":
			_show_tab(studio.BACKTEST, "已開啟回測設定。")
		"import":
			_show_tab(studio.CODE, "請選擇要匯入的 PowerLanguage 檔案。")
			wb._choose("import")
		"new", "draft":
			draft_source = str(intent.get("source","")) if action == "draft" else ""
			if action == "draft" and (draft_source.strip_edges().is_empty() or draft_source.to_utf8_buffer().size()>100000):
				_finish("沒有收到可放入草稿的完整 PowerLanguage。")
				return
			var args := {"number":wb.documents.size()+1}
			if action=="draft": args.ai_source=draft_source
			_start_workbench("new",args)
		"run", "check", "save":
			pending = action
			if action == "run":
				wb._run()
			else:
				wb._document_action("check")
			if not wb.busy:
				_finish(wb.status.text, studio.BACKTEST, "檢視回測設定")
			else:
				_sync_send()
				if action=="run" and telegram_reply.is_valid():
					telegram_reply.call("回測已開始，完成後會回傳摘要。")
		"report":
			_show_report(wb.documents[wb.active].get("result"))
		"optimize":
			_start_optimizer(intent)
		_:
			_finish("未辨識到可執行的工作台操作。")

func _start_workbench(operation: String, args: Dictionary) -> void:
	pending = operation
	studio.workbench._request(operation,args)
	if not studio.workbench.busy:
		_finish("無法啟動這項工作台操作。")
	else:
		_sync_send()

func workbench_finished(operation: String, data: Variant, error: String) -> void:
	if pending != operation:
		return
	if not error.is_empty():
		_finish(error)
		return
	var wb: Control = studio.workbench
	match operation:
		"open":
			var next := after_open
			after_open = {}
			_resume_workbench.call_deferred(next)
		"new":
			if not draft_source.is_empty():
				wb.editor.text = draft_source
				wb._source_changed()
				wb._checkpoint()
				_finish("已另開 PowerLanguage 草稿。可先檢查語法，再使用回測驗證。", studio.CODE, "檢視草稿")
			else:
				_finish("已建立空白策略草稿。", studio.CODE, "開啟草稿")
		"check":
			_finish(str(data.get("message","語法檢查完成。")), studio.CODE, "開啟程式碼")
		"save":
			_finish("策略副本已儲存。", studio.CODE, "查看策略")
		"run":
			_show_report(data.receipt)

func _resume_workbench(next: Dictionary) -> void:
	var wb: Control = studio.workbench
	while wb.busy:
		await get_tree().process_frame
	_execute(next)

func _show_report(receipt: Variant) -> void:
	if not receipt is Dictionary:
		_finish("目前策略尚無回測報表。可輸入「開始回測」。")
		return
	if receipt.get("status") != "completed":
		_finish("回測未完成。%s" % str(receipt.get("error","請查看執行結果。")), studio.BACKTEST, "查看回測結果", true)
		return
	var s: Dictionary = receipt.summary
	var text := "回測 %s\n已平倉淨利 %.2f · 最大回撤 %.2f\n已平倉 %d 筆 · 勝率 %.2f%%" % [
		receipt.run_id,s.net_closed,s.max_drawdown,int(s.trades),s.win_rate]
	if studio.workbench.run_info.text.begins_with("PL或設定已修改"):
		text += "\n策略或設定已變更。上述數值來自前次執行。"
	_finish(text, studio.BACKTEST, "開啟完整報表", true)

func _start_optimizer(intent: Dictionary) -> void:
	var wb: Control = studio.workbench
	var goal := str(intent.get("goal","")).strip_edges()
	if goal.is_empty():
		_finish("請補充修改目標，例如「優化目前策略：減少盤整交易，保留原有停損」。", studio.OPTIMIZE, "開啟 AI 優化")
		return
	var rounds := int(intent.get("rounds",0))
	if rounds < 0 or rounds > 5:
		_finish("單次 AI 優化可執行 1 至 5 輪。請重新指定輪數。", studio.OPTIMIZE, "開啟 AI 優化")
		return
	wb.optimizer.goal.text = goal
	if rounds > 0:
		wb.optimizer.rounds.value = rounds
	if intent.get("objective") in ["drawdown","profit"]:
		wb.optimizer.objective.select(1 if intent.objective=="drawdown" else 0)
	wb._start_optimization()
	if not wb.optimizer.running:
		_finish(wb.optimizer.detail.text, studio.OPTIMIZE, "開啟 AI 優化")
		return
	pending = "optimizer"
	_sync_send()
	while wb.optimizer.running:
		await get_tree().process_frame
	_finish(wb.optimizer.detail.text, studio.OPTIMIZE, "檢視候選版本")

func _start_market(action: String, symbol: String) -> void:
	market_action = action
	market_symbol = symbol.strip_edges().to_upper()
	studio.ensure_quote()
	var quote: Control = studio.quote
	if quote.busy and quote.action != "inventory":
		_finish("行情資料庫正在執行其他作業。")
	elif quote.busy:
		pending = "inventory"
		_sync_send()
	else:
		_market_inventory(quote.inventory)

func _market_inventory(inventory: Array) -> void:
	if market_symbol.is_empty():
		market_symbol = studio.quote.selected_symbol
	for entry in inventory:
		if entry.instrument.symbol.nocasecmp_to(market_symbol) != 0:
			continue
		if market_action == "coverage":
			if studio.quote.dirty:
				_finish("請先儲存商品設定的修改。", studio.MARKET, "開啟行情資料庫")
				return
			studio.quote._select_symbol(market_symbol)
			pending = "coverage_all"
			_sync_send()
			studio.quote._operation("coverage")
			if not studio.quote.busy:
				_finish(studio.quote.status.text, studio.MARKET, "查看行情資料庫")
			return
		var lines := PackedStringArray()
		for source in entry.sources:
			lines.append("%s · %d 筆\n%s → %s" % [
				{"tick":"Tick","minute":"1 分 K","daily":"日 K"}.get(source.kind,source.kind),
				int(source.row_count),DT.date_time(source.get("first_datetime","")),DT.date_time(source.get("last_datetime",""))])
		_finish("%s 行情來源\n%s" % [market_symbol,"\n".join(lines) if not lines.is_empty() else "尚無行情資料。"], studio.MARKET, "開啟行情資料庫")
		return
	_finish("資料庫找不到商品「%s」。" % market_symbol, studio.MARKET, "開啟行情資料庫")

func market_finished(operation: String, data: Variant, error: String) -> void:
	if pending != operation:
		return
	if not error.is_empty():
		_finish(error, studio.MARKET, "開啟行情資料庫")
	elif operation == "inventory":
		_market_inventory(data)
	elif operation == "coverage_all":
		var lines := PackedStringArray()
		for source in data.sources:
			if source.kind == "tick":
				lines.append("Tick · 疑似缺口 %d 段" % int(source.gap_count))
			else:
				lines.append("%s · 缺少 %d 根" % ["1 分 K" if source.kind=="minute" else "日 K",int(source.missing)])
		_finish("%s 完整性檢查完成\n%s" % [market_symbol,"\n".join(lines)], studio.MARKET, "檢視完整性圖表")

func _ask_model(prompt: String) -> void:
	var wb: Control = studio.workbench
	var choice := _model_selection()
	var connection: Dictionary = wb.optimizer.connection_settings(choice.get("provider",""),choice.get("model",""))
	request_provider=connection.provider
	var key: String = connection.key
	if key.is_empty() or "\r" in key or "\n" in key:
		_finish("請先在設定頁填入 %s API 金鑰。" % AI.NAMES[request_provider], studio.SETTINGS, "開啟設定")
		return
	var acceptance_fields := {}
	for criterion in ["market","trade_rules","backtest","pass_criteria","evidence",
		"product","bar_period","market_session","strategy_goal","entry_rules","exit_rules","stop_loss","take_profit"]:
		acceptance_fields[criterion]={"type":"string"}
	var properties := {"action":{"type":"string","enum":ACTIONS},"rounds":{"type":"integer"},
		"review_mode":{"type":"string","enum":["","new","existing"]},
		"acceptance":{"type":"object","additionalProperties":false,"properties":acceptance_fields,
			"required":acceptance_fields.keys()}}
	for field in ["strategy","symbol","goal","source","message","objective","research_action"]:
		properties[field] = {"type":"string"}
	model_prompt=prompt
	var active: Dictionary = wb.documents[wb.active] if wb.active >= 0 else {}
	model_document_id = active.get("id","")
	var input := {"prompt":prompt,"known_strategy":_match_strategy(prompt),"active_context":context.text}
	model_context=_research_context().duplicate(true)
	input.strategy_context=model_context
	var named_strategy: String=input.known_strategy
	if named_strategy.is_empty() and not research_review.is_empty() and research_review.remote==telegram_reply.is_valid():
		named_strategy=_match_strategy(research_review.history.front().user)
	var named_entries: Array=wb.catalog.filter(func(entry: Dictionary):
		return entry.kind=="strategy" and (named_strategy.nocasecmp_to(entry.id)==0 or named_strategy.nocasecmp_to(entry.name.trim_prefix("REA_"))==0))
	if named_entries.size()==1:
		var entry: Dictionary=named_entries[0]
		var task_path: Variant=entry.get("task")
		if task_path is String and not task_path.is_empty() and FileAccess.file_exists(task_path):
			var saved: Variant=JSON.parse_string(FileAccess.get_file_as_string(task_path))
			if saved is Dictionary:
				input.named_strategy_context={"name":entry.name,"market":saved.get("market"),"parameters":saved.get("parameters")}
	if not research_review.is_empty() and research_review.remote==telegram_reply.is_valid():
		input.review_history=research_review.history
		input.acceptance=research_review.get("acceptance",{})
	if not approved_review.is_empty():
		input.confirmed_acceptance=approved_review
	if ("報表" in prompt or "績效" in prompt) and active.get("result") is Dictionary:
		var receipt: Dictionary = active.result
		if receipt.get("status") == "completed":
			input.report = {"run_id":receipt.run_id,"summary":receipt.summary}
	var payload := {
		"model":connection.model,"store":false,"max_output_tokens":8000,
		"instructions":"用繁體中文。將提示詞轉成一個工作台操作。market 開行情，sources 查商品資料，coverage 查完整性，market_import 與 market_export 開行情檔案流程，code 開編輯器，open 開指定策略，new 開空白策略，import 開 PL 選檔，save 儲存策略副本，check 檢查語法，backtest 開回測設定，run 執行回測，report 摘要報表，optimize 依 goal 自動優化，api 開 API 設定，tradecom 開 TradeCom 設定，draft 依明確請求產生完整 PowerLanguage Signal 草稿，reply 回覆一般詢問。策略研究條件不完整時使用 clarify。查詢策略庫的策略數量、清單或是否存在時，用 strategy_catalog，strategy 填查詢的策略名稱或商品關鍵字（例如日經），查全部時留空。其他操作的 strategy 僅填明確指定的策略名。objective 可用 profit 或 drawdown，rounds 未指定填 0。draft 的 source 是原碼，其他動作留空。未使用的字串填空。使用者明確拒絕操作或詢問其他資訊時，用 reply。若輸入含 report，僅用該實際數據說明績效。不得宣稱執行任何動作。run 使用既有設定，若要求改參數則用 backtest。禁止下單、修改憑證或執行任意命令。",
		"input":JSON.stringify(input),
		"text":{"format":{"type":"json_schema","name":"workbench_action","strict":true,
			"schema":{"type":"object","additionalProperties":false,"properties":properties,"required":properties.keys()}}}
	}
	payload.instructions += "\n"+GRILL_ME+"\n"+REVIEW_ROUTING
	payload.instructions += "\n產出的 PowerLanguage 程式碼須用繁體中文註釋說明重要參數與進出場條件，文字須可用 Big5／CP950 編碼，避免表情符號與罕用字。"
	pending = "model"
	_sync_send()
	if request.request(AI.endpoint(request_provider,connection.model),AI.headers(request_provider,key),
		HTTPClient.METHOD_POST,JSON.stringify(AI.payload_for(request_provider,payload))) != OK:
		_finish("無法送出 %s 請求。" % AI.NAMES[request_provider])

func _model_done(result: int, code: int, _headers: PackedStringArray, body: PackedByteArray) -> void:
	if pending != "model":
		return
	if result != HTTPRequest.RESULT_SUCCESS or code != 200:
		_finish("%s 請求未完成（HTTP %d）。請確認連線、金鑰及模型設定。" % [AI.NAMES[request_provider],code])
		return
	var intent: Variant=AI.read_json(request_provider,body)
	if not intent is Dictionary or intent.get("action") not in ACTIONS:
		_finish("未收到可執行的工作台操作，請改用較簡短的提示詞。")
		return
	var wb: Control = studio.workbench
	var active: Dictionary = wb.documents[wb.active] if wb.active >= 0 else {}
	if model_document_id != active.get("id",""):
		_finish("目前策略在等待回覆時已切換，請重新送出提示詞。")
		return
	if intent.action in ["clarify","research_plan"]:
		if model_context!=_research_context():
			_finish("策略或回測設定已變更，請重新送出原需求。")
			return
		_request_requirements(intent)
		return
	if intent.action in RESEARCH_ACTIONS:
		if approved_review.is_empty() or approved_review.action!=intent.action or approved_review.context!=_research_context():
			_finish("策略作業仍需要可驗收的項目與你的確認。請重新送出需求。")
			return
		for key in ["strategy","goal","objective","rounds"]:
			intent[key]=approved_review.intent.get(key,0 if key=="rounds" else "")
		if intent.action=="optimize":
			intent.goal=str(intent.goal)+"\n已確認的驗收項目\n"+str(approved_review.history.back().review)
		approved_review.clear()
		_execute(intent)
		return
	if not approved_review.is_empty():
		_finish("模型未產生已確認的策略操作，請重新送出需求。")
		return
	if intent.action in ["save","coverage"]:
		if telegram_reply.is_valid():
			_finish("模型建議的操作需要確認。請在 Telegram 明確輸入指令，例如「開始回測」或「檢查 TXF1 行情完整性」。")
			return
		pending = "confirm"
		var description: String = {"run":"回測","optimize":"AI 優化","save":"儲存策略副本","coverage":"檢查行情完整性","draft":"建立 PL 草稿"}[intent.action]
		var target: String = str(intent.get("symbol","")) if intent.action == "coverage" else str(intent.get("strategy",""))
		if not target.is_empty():
			description += " · " + target
		if intent.action == "optimize" and not str(intent.goal).is_empty():
			description += "\n" + str(intent.goal)
		_bubble(false, "準備執行「%s」。" % description)
		var card: VBoxContainer = transcript.get_child(-1).get_child(0)
		var controls := _row(card)
		_button(controls, "執行", func():
			controls.hide()
			var current: Dictionary = wb.documents[wb.active] if wb.active >= 0 else {}
			if model_document_id != current.get("id",""):
				_finish("目前策略已切換，請重新送出提示詞。")
				return
			_execute(intent), true)
		_button(controls, "取消", func():
			controls.hide()
			_finish("已取消。"))
		_sync_send()
		return
	_execute(intent)

func _request_requirements(intent: Dictionary) -> void:
	var remote := telegram_reply.is_valid()
	if not research_review.is_empty() and research_review.remote!=remote:
		_finish("另一個對話還在確認策略需求，請先在原對話完成或取消。")
		return
	var wb: Control=studio.workbench
	var payload := {"root":wb.root,"action":"strategy_requirements"}
	payload.request=model_prompt if research_review.is_empty() else research_review.history.front().user
	var mode: String=research_review.get("mode",intent.get("review_mode",""))
	if mode in ["new","existing"]: payload.mode=mode
	var target: String=str(intent.get("strategy","")).strip_edges()
	if target.is_empty(): target=_match_strategy(model_prompt)
	if target.is_empty() and not research_review.is_empty():
		target=str(research_review.intent.get("strategy",""))
	if target.is_empty() and wb.active>=0 and wb.documents[wb.active].kind=="strategy":
		payload.document=wb.documents[wb.active]
	elif not target.is_empty():
		var matches: Array=wb.catalog.filter(func(entry: Dictionary):
			return entry.kind=="strategy" and (target.nocasecmp_to(entry.id)==0 or target.nocasecmp_to(entry.name.trim_prefix("REA_"))==0))
		if matches.size()==1: payload.entry=matches[0]
	var acceptance: Dictionary=research_review.get("acceptance",{}).duplicate()
	var extracted: Variant=intent.get("acceptance",{})
	if extracted is Dictionary:
		for key in extracted:
			if extracted[key] is String and not extracted[key].strip_edges().is_empty() and not acceptance.has(key):
				acceptance[key]=extracted[key]
	payload.acceptance=acceptance
	if requirements_bridge==null: requirements_bridge=ClassDB.instantiate("WorkbenchBridge")
	if requirements_bridge==null or not requirements_bridge.start(JSON.stringify(payload)):
		_finish("無法取得策略 MCP 的驗收問題。")
		return
	requirements_intent=intent
	requirements_acceptance=acceptance
	pending="requirements"
	_sync_send()

func _poll_requirements() -> void:
	var raw: String=requirements_bridge.poll()
	if raw.is_empty(): return
	var response: Variant=JSON.parse_string(raw)
	if not response is Dictionary or not response.get("ok",false):
		_finish("無法取得策略 MCP 的驗收問題。"+str(response.get("error","") if response is Dictionary else ""))
		return
	if model_context!=_research_context():
		_finish("策略或回測設定已變更，請重新送出原需求。")
		return
	_show_research_review(requirements_intent,response.data,requirements_acceptance)

func _show_research_review(intent: Dictionary, review: Dictionary, acceptance: Dictionary) -> void:
	var remote := telegram_reply.is_valid()
	var questions: Array=review.questions
	var action: String=str(intent.get("research_action","")) if questions.is_empty() else ""
	var review_text := ""
	for index in range(questions.size()):
		var question: Dictionary=questions[index]
		review_text+="\n%d. %s" % [index+1,question.question]
		for option: Dictionary in question.options:
			review_text+="\n   "+("建議 · " if option.recommended else "")+option.label
	if questions.is_empty():
		if action not in RESEARCH_ACTIONS:
			_finish("請指定要撰寫策略、優化或執行回測。")
			return
		for item: Dictionary in review.checklist:
			review_text+="\n%s｜%s" % [item.item,item.criterion]
		review_text+="\n"+review.mandatory
	var history: Array=research_review.get("history",[])
	history.append({"user":model_prompt,"review":review_text.strip_edges()})
	research_review={"action":action,"context":_research_context().duplicate(true),"history":history,
		"remote":remote,"mode":review.get("mode","existing"),"acceptance":acceptance,"intent":{"strategy":intent.get("strategy",""),"goal":intent.get("goal",""),
		"objective":intent.get("objective",""),"rounds":intent.get("rounds",0)}}
	if action.is_empty():
		_finish(review_text.strip_edges() if remote else "請回答 MCP 回傳的問題。")
		if not remote and not questions.is_empty(): _add_review_choices(questions)
	else:
		_finish("待確認的策略驗收項目\n"+review_text.strip_edges()+"\n\n同意請回覆「確認驗收」。可直接提出修改，或回覆「取消驗收」。")

func _add_review_choices(questions: Array) -> void:
	if is_instance_valid(review_form): review_form.hide()
	var card: VBoxContainer=transcript.get_child(-1).get_child(0)
	review_form=VBoxContainer.new()
	review_form.name="ReviewChoices"
	card.add_child(review_form)
	var form := review_form
	var fields: Array=[]
	for index in range(questions.size()):
		var question: Dictionary=questions[index]
		var label := _label(form,"%d. %s" % [index+1,question.question],15)
		label.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
		var picker := OptionButton.new()
		picker.fit_to_longest_item=false
		picker.add_item("請選擇答案")
		picker.set_item_disabled(0,true)
		for option: Dictionary in question.options:
			var prefix := "建議 · " if option.recommended else ""
			picker.add_item(prefix+option.label)
			picker.set_item_metadata(picker.item_count-1,option)
		picker.select(0)
		form.add_child(picker)
		var custom := LineEdit.new()
		custom.placeholder_text="輸入你的答案或補充條件"
		custom.max_length=1000
		custom.hide()
		form.add_child(custom)
		picker.item_selected.connect(func(selected: int):
			custom.visible=picker.get_item_metadata(selected).free_text
			if custom.visible: custom.grab_focus())
		fields.append({"key":question.key,"question":question.question,"picker":picker,"custom":custom})
	var validation := _label(form,"",14,MUTED)
	_button(form,"送出本輪答案",func():
		if not pending.is_empty() or not form.is_visible_in_tree() or form!=review_form: return
		var answer := _review_answers(fields)
		if answer.is_empty():
			validation.text="請回答每一題。選擇「自行描述」時，請填寫內容。"
			return
		var lines := PackedStringArray()
		for field: Dictionary in fields:
			lines.append("%s\n我的答案：%s" % [field.question,answer[field.key]])
		research_review.acceptance.merge(answer,true)
		form.hide()
		_submit_text("\n".join(lines)),true)

func _review_answers(fields: Array) -> Dictionary:
	var answers := {}
	for field: Dictionary in fields:
		var picker: OptionButton=field.picker
		if picker.selected<=0: return {}
		var option: Dictionary=picker.get_item_metadata(picker.selected)
		var answer: String=field.custom.text.strip_edges() if option.free_text else option.label
		if answer.is_empty(): return {}
		answers[field.key]=answer
	return answers
