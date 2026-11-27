extends Node

signal state_changed(message: String, active: bool)
signal prompt_received(prompt: String, reply: Callable)

const API := "https://api.telegram.org/bot"
const POLL_SECONDS := 25
const HELP := "工作台已連線。私訊可直接輸入提示詞，群組請用 /ask@機器人名稱 加上提示詞。\n可試「開始回測」、「摘要最近回測報表」或「查詢 TXF1 行情來源」。\n傳送 /ping 可測試收發。"

var receiver: HTTPRequest
var sender: HTTPRequest
var reconnect: Timer
var delivery_timer: Timer
var token := ""
var configured_chat := ""
var chat_id := 0
var chat_type := ""
var username := ""
var generation := 0
var offset := 0
var phase := ""
var active := false
var listening := false
var test_requested := false
var outgoing: Array[Dictionary] = []
var sending := {}
var config_loader: Callable
var retrying := false

func _ready() -> void:
	receiver = _client(POLL_SECONDS + 12, _received)
	sender = _client(15, _sent)
	reconnect = Timer.new()
	reconnect.one_shot = true
	reconnect.timeout.connect(_poll)
	add_child(reconnect)
	delivery_timer = Timer.new()
	delivery_timer.one_shot = true
	delivery_timer.timeout.connect(_send_next)
	add_child(delivery_timer)

func _client(timeout: int, done: Callable) -> HTTPRequest:
	var client := HTTPRequest.new()
	client.timeout = timeout
	client.max_redirects = 0
	client.body_size_limit = 2_000_000
	client.request_completed.connect(done)
	add_child(client)
	return client

func start(config: Dictionary, test := false) -> void:
	var candidate: String = str(config.get("token","")) if config.get("token") is String else ""
	var target: String = str(config.get("chat_id","")).strip_edges()
	if active and candidate == token and target == configured_chat:
		if test:
			_request_test()
		return
	stop()
	if candidate.is_empty() or target.is_empty():
		state_changed.emit("請先儲存 Chat ID 和 Bot Token。",false)
		return
	var pattern := RegEx.new()
	pattern.compile("^[0-9]+:[A-Za-z0-9_-]+$")
	if pattern.search(candidate)==null:
		state_changed.emit("Bot Token 格式不符，請核對 BotFather 提供的完整權杖。",false)
		return
	token=candidate
	configured_chat=target
	active=true
	test_requested=test
	state_changed.emit("正在驗證 Telegram 機器人與對話…",true)
	_receive("getMe",{})

func stop(message := "Telegram 收訊已停止。") -> void:
	generation+=1
	active=false
	listening=false
	retrying=false
	test_requested=false
	phase=""
	token=""
	configured_chat=""
	chat_id=0
	chat_type=""
	username=""
	offset=0
	outgoing.clear()
	sending={}
	receiver.cancel_request()
	sender.cancel_request()
	reconnect.stop()
	delivery_timer.stop()
	state_changed.emit(message,false)

func settings_changed(config: Dictionary) -> void:
	if active and (config.get("token","")!=token or config.get("chat_id","")!=configured_chat):
		stop("Telegram 設定已變更，請重新開始收訊。")

func _post(client: HTTPRequest, method: String, body: Dictionary) -> Error:
	return client.request(API + token + "/" + method,
		["Content-Type: application/json"], HTTPClient.METHOD_POST,JSON.stringify(body))

func _receive(method: String, body: Dictionary, stage := "") -> void:
	phase=stage if not stage.is_empty() else method
	if _post(receiver,method,body)!=OK:
		if phase=="poll":
			_retry("Telegram 收訊連線中斷，正在重試。",4)
		else:
			stop("無法連線 Telegram Bot API。")

func _received(result: int, status: int, _headers: PackedStringArray, raw: PackedByteArray) -> void:
	if not active:
		return
	if phase=="poll" and not _config_current():
		return
	var response := _decode(result,status,raw)
	if not response.ok:
		if phase=="poll" and (status==0 or status==429 or status>=500):
			_retry("Telegram 收訊暫時中斷，正在重試。",response.retry_after)
		else:
			stop(_connection_error(status))
		return
	match phase:
		"getMe":
			if not response.data is Dictionary or not response.data.get("is_bot",false):
				stop("Telegram 未回傳有效的機器人資訊。")
				return
			username=str(response.data.get("username",""))
			var target: Variant=int(configured_chat) if configured_chat.is_valid_int() else configured_chat
			_receive("getChat",{"chat_id":target})
		"getChat":
			if not response.data is Dictionary or int(response.data.get("id",0))==0:
				stop("Telegram 未回傳有效的 Chat ID。")
				return
			chat_id=int(response.data.id)
			chat_type=str(response.data.get("type",""))
			if chat_type not in ["private","group","supergroup"]:
				stop("此對話類型無法接收工作台指令。請使用機器人私訊或群組。")
				return
			_receive("getWebhookInfo",{})
		"getWebhookInfo":
			if not response.data is Dictionary:
				stop("無法確認 Telegram 接收方式。")
				return
			if not str(response.data.get("url","")).is_empty():
				if test_requested:
					test_requested=false
					_queue("策略研究工作台 Telegram 發訊測試。",{},"webhook_test")
				else:
					stop("此機器人已有 Webhook。工作台需使用獨立的機器人或由你調整既有接收端。")
				return
			_receive("getUpdates",{"offset":-1,"limit":1,"timeout":0,"allowed_updates":["message"]},"bootstrap")
		"bootstrap":
			_update_offset(response.data)
			listening=true
			state_changed.emit("Telegram 收訊中。可傳送 /ping 測試。",true)
			if test_requested:
				_request_test()
			_poll.call_deferred()
		"poll":
			if retrying:
				retrying=false
				state_changed.emit("Telegram 已恢復收訊。",true)
			var updates: Array=response.data if response.data is Array else []
			updates.sort_custom(func(a: Dictionary,b: Dictionary): return int(a.update_id)<int(b.update_id))
			for update in updates:
				if int(update.get("update_id",-1))<offset:
					continue
				_update_offset([update])
				_handle_update(update)
			_poll.call_deferred()

func _decode(result: int, status: int, raw: PackedByteArray) -> Dictionary:
	var answer: Variant=JSON.parse_string(raw.get_string_from_utf8()) if not raw.is_empty() else null
	if not answer is Dictionary:
		return {"ok":false,"retry_after":4}
	if result!=HTTPRequest.RESULT_SUCCESS or status!=200 or not answer.get("ok",false):
		return {"ok":false,"retry_after":clampi(int(answer.get("parameters",{}).get("retry_after",4)),2,120)}
	return {"ok":true,"data":answer.get("result")}

func _connection_error(status: int) -> String:
	match status:
		401,404: return "Telegram 驗證失敗。請核對 Bot Token。"
		400,403: return "Telegram 無法存取設定的對話。請核對 Chat ID，並先啟動機器人私訊或將機器人加到群組。"
		409: return "此機器人已有其他輪詢程序或 Webhook。請停止另一個接收端後再開始。"
		_: return "Telegram Bot API 連線失敗（HTTP %d）。" % status

func _retry(message: String, seconds: int) -> void:
	retrying=true
	state_changed.emit(message,true)
	reconnect.start(seconds)

func _poll() -> void:
	if active and listening:
		_receive("getUpdates",{"offset":offset,"timeout":POLL_SECONDS,"allowed_updates":["message"]},"poll")

func _update_offset(updates: Variant) -> void:
	if updates is Array:
		for update in updates:
			offset=maxi(offset,int(update.get("update_id",-1))+1)

func _request_test() -> void:
	test_requested=not listening
	if listening:
		_queue("策略研究工作台測試訊息。請回傳 /ping 測試收訊。",{}, "test")

func _handle_update(update: Dictionary) -> void:
	var message: Dictionary=update.get("message",{})
	var author: Dictionary=message.get("from",{})
	if int(message.get("chat",{}).get("id",0))!=chat_id or author.is_empty() or author.get("is_bot",false):
		return
	var text: String=str(message.get("text","")).strip_edges()
	if text.is_empty():
		return
	var reply_to := {"message_id":int(message.message_id),"thread_id":int(message.get("message_thread_id",0))}
	if text.begins_with("/"):
		var space := text.find(" ")
		var command := text if space==-1 else text.left(space)
		var command_parts := command.to_lower().split("@",false,1)
		if command_parts.size()>1 and command_parts[1]!=username.to_lower():
			return
		match command_parts[0]:
			"/ping":
				_queue("Pong · 工作台已收到 Telegram 訊息。",reply_to,"ping")
				return
			"/start", "/help":
				_queue(HELP,reply_to)
				return
			"/ask":
				text=text.substr(command.length()).strip_edges()
			_:
				_queue("請使用 /ask 加上提示詞，或用 /ping 測試。",reply_to)
				return
	elif chat_type!="private":
		return
	if text.is_empty():
		_queue("請在 /ask 後面輸入提示詞。",reply_to)
		return
	var session := generation
	prompt_received.emit(text,func(answer: String):
		if listening and session==generation:
			_queue(answer,reply_to))

func _queue(text: String, reply_to: Dictionary, tag := "") -> void:
	if not _config_current() or outgoing.size()>=32:
		return
	var content := {"chat_id":chat_id,"text":text.left(1900)}
	if int(reply_to.get("message_id",0))>0:
		content.reply_parameters={"message_id":reply_to.message_id,"allow_sending_without_reply":true}
	if int(reply_to.get("thread_id",0))>0:
		content.message_thread_id=reply_to.thread_id
	outgoing.append({"content":content,"tag":tag,"retry":0})
	_send_next()

func _config_current() -> bool:
	if active and config_loader.is_valid():
		settings_changed(config_loader.call())
	return active

func _send_next() -> void:
	if not active or not sending.is_empty() or outgoing.is_empty() or not delivery_timer.is_stopped():
		return
	sending=outgoing.pop_front()
	if _post(sender,"sendMessage",sending.content)!=OK:
		sending={}
		state_changed.emit("無法送出 Telegram 回覆。",true)
		_send_next.call_deferred()

func _sent(result: int, status: int, _headers: PackedStringArray, raw: PackedByteArray) -> void:
	if not active or sending.is_empty():
		return
	var response := _decode(result,status,raw)
	var sent := sending
	sending={}
	if sent.tag=="webhook_test":
		stop("Telegram 已接受測試訊息。此機器人已有 Webhook，工作台尚未收訊。" if response.ok else _connection_error(status))
		return
	if not response.ok:
		if status==429 and sent.retry==0:
			sent.retry=1
			outgoing.push_front(sent)
			delivery_timer.start(response.retry_after)
			return
		state_changed.emit("Telegram 訊息未送出。%s" % _connection_error(status),true)
	elif sent.tag=="test":
		state_changed.emit("Telegram 已接受測試訊息。請傳送 /ping 驗證接收。",true)
	elif sent.tag=="ping":
		state_changed.emit("Telegram 收發測試通過，已收到 /ping 並送出 Pong。",true)
	_send_next.call_deferred()
