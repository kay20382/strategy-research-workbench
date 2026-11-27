extends "res://skin.gd"

var dialog: AcceptDialog
var confirmation: ConfirmationDialog
var bridge: RefCounted
var mode: OptionButton
var connect_button: Button
var send_button: Button
var status: Label
var side: OptionButton
var effect: OptionButton
var price_type: OptionButton
var duration: OptionButton
var price: LineEdit
var quantity: SpinBox
var profile := ""
var connected := false
var broker_ready := false
var recovered := false
var awaiting := ""
var needs_broker_review := false
var pending := {}
var sequence := 0

func _ready() -> void:
	dialog=AcceptDialog.new()
	dialog.title="凱基海外期貨委託"
	dialog.ok_button_text="關閉視窗"
	add_child(dialog)
	var form := VBoxContainer.new()
	form.add_theme_constant_override("separation",8)
	dialog.add_child(form)
	var connection := _row(form)
	mode=_options(connection,["模擬","正式交易"])
	connect_button=_button(connection,"連線",_connect)
	status=_label(form,"尚未連線",14,MUTED)
	status.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	_label(form,"商品  ·  225M 小型日經 225 連續月",15,INK)
	var order := _row(form)
	side=_options(order,["買進","賣出"])
	effect=_options(order,["新倉","平倉"])
	price_type=_options(order,["限價","市價"])
	duration=_options(order,["IOC","FOK","ROD"])
	var amounts := _row(form)
	price=_field(amounts,"限價")
	price_type.item_selected.connect(func(index): price.editable=index==0)
	_label(amounts,"口數",14,MUTED)
	quantity=SpinBox.new()
	quantity.min_value=1
	quantity.max_value=65535
	quantity.value=1
	amounts.add_child(quantity)
	send_button=_button(form,"送出海外委託",_prepare,true)
	_label(form,"委託與成交可在交易回報查核。改撤單及接續前的部位請使用券商交易平台。",13,MUTED)
	confirmation=ConfirmationDialog.new()
	confirmation.title="確認正式海外期貨委託"
	confirmation.ok_button_text="確認送出"
	confirmation.confirmed.connect(_submit)
	confirmation.canceled.connect(func(): pending={})
	dialog.add_child(confirmation)
	if ClassDB.class_exists("TradeBridge"): bridge=ClassDB.instantiate("TradeBridge")
	var timer := Timer.new()
	timer.wait_time=0.2
	timer.timeout.connect(_poll)
	add_child(timer)
	timer.start()
	_sync()

func _options(parent: Container, choices: Array) -> OptionButton:
	var options := OptionButton.new()
	parent.add_child(options)
	for choice in choices: options.add_item(choice)
	return options

func open_for(selected_profile: String) -> void:
	if not connected:
		profile=selected_profile
		status.text="正式帳戶：%s" % ("尚未選擇" if profile.is_empty() else profile)
	dialog.popup_centered(Vector2i(570,390))

func _resolve_nikkei() -> String:
	var reply: Dictionary=JSON.parse_string(bridge.nikkei_quarter())
	if not reply.ok:
		status.text="暫時無法確認日經連續商品。"
		return ""
	return reply.data.maturity

func _sync() -> void:
	mode.disabled=connected
	connect_button.text="中斷連線" if connected else "連線"
	send_button.disabled=not broker_ready or not recovered or not awaiting.is_empty()

func _connect() -> void:
	if bridge==null:
		status.text="找不到交易模組。"
		return
	if connected:
		bridge.stop()
		connected=false
		broker_ready=false
		recovered=false
		status.text="交易連線已關閉。請在交易回報核對委託與成交。"
		_sync()
		return
	if mode.selected==1 and profile.is_empty():
		status.text="請先選擇凱基 TradeCom 帳戶。"
		return
	var reply: Dictionary=JSON.parse_string(bridge.start_foreign(profile,"paper" if mode.selected==0 else "live"))
	if not reply.ok:
		status.text=reply.error
		return
	connected=true
	awaiting=""
	status.text="正在建立海外期貨連線…"
	_sync()

func _prepare() -> void:
	if not broker_ready or not recovered or not awaiting.is_empty(): return
	var current := _resolve_nikkei()
	if current.is_empty(): return
	if price_type.selected==0 and (not price.text.is_valid_float() or float(price.text)<=0):
		status.text="請填入有效的正數限價。"
		return
	if price_type.selected==1 and duration.selected==2:
		status.text="市價委託須選 IOC 或 FOK。"
		return
	sequence+=1
	pending={"id":"foreign-%d-%d-%d" % [int(Time.get_unix_time_from_system()*1000),OS.get_process_id(),sequence],
		"action":"place","exchange":"JPX","product":"NK225M","maturity":current,
		"side":["buy","sell"][side.selected],"position_effect":["open","close"][effect.selected],
		"price_type":["limit","market"][price_type.selected],"price":price.text.strip_edges(),
		"time_in_force":["ioc","fok","rod"][duration.selected],"quantity":str(int(quantity.value))}
	if mode.selected==0:
		_submit()
		return
	confirmation.dialog_text="帳戶設定 %s\n225M 小型日經 225 連續月\n%s・%s・%s 口・%s %s" % [
		profile,side.text,effect.text,pending.quantity,
		price.text if price_type.selected==0 else "市價",duration.text]
	confirmation.popup_centered(Vector2i(475,180))

func _submit() -> void:
	if pending.is_empty() or not broker_ready or not recovered or not awaiting.is_empty(): return
	if pending.maturity!=_resolve_nikkei():
		pending={}
		status.text="行情對應已更新，請重新確認委託。"
		return
	pending.confirmed=mode.selected==1
	var reply: Dictionary=JSON.parse_string(bridge.submit(JSON.stringify(pending)))
	if reply.ok:
		awaiting=pending.id
		status.text="請求已送出，等待回報。"
	else:
		status.text=reply.error
	pending={}
	_sync()

func _poll() -> void:
	if not connected: return
	var reply: Dictionary=JSON.parse_string(bridge.poll())
	if not reply.ok:
		status.text=reply.error
		connected=false
		broker_ready=false
		recovered=false
		_sync()
		return
	for event in reply.data.events:
		var data: Dictionary=event.data
		if event.type=="status":
			status.text=data.message
			if data.state in ["reconnecting","disconnected","error"]:
				needs_broker_review=needs_broker_review or not awaiting.is_empty()
				awaiting=""
				pending={}
				confirmation.hide()
				broker_ready=false
				recovered=false
			elif data.state=="ready":
				broker_ready=true
				if mode.selected==0: recovered=true
		elif event.type=="recovery" and data.state=="complete":
			recovered=true
			status.text="海外回報回補完成。請先核對中斷前委託。" if needs_broker_review else "海外回報回補完成，可送出委託。"
		elif not awaiting.is_empty() and data.get("id")==awaiting:
			if event.type=="reply":
				status.text=data.message
				if data.state=="rejected": awaiting=""
			elif event.type=="order_report" or event.type=="fill":
				status.text=("模擬委託回報" if data.get("simulated",false) else "已收到券商回報")+" · "+str(data.get("order_no",""))
				awaiting=""
			elif (event.type=="order_ack" and int(data.code)!=0) or (event.type=="transport_ack" and not data.received):
				status.text="券商拒絕或傳輸未確認，請先核對交易回報。"
				if event.type=="order_ack": awaiting=""
	if not reply.data.active:
		connected=false
		broker_ready=false
		recovered=false
		status.text="連線已結束。請先核對交易回報。"
	_sync()
