extends SceneTree

const Studio = preload("res://studio.tscn")
const PL = preload("res://optimizer.gd")
const Bot = preload("res://telegram_bot.gd")
const AI = preload("res://ai_api.gd")

class StubTelegram extends Bot:
	var sent_messages: Array[Dictionary]=[]
	var latest_poll := {}
	var last_state := ""

	func _post(client: HTTPRequest, method: String, body: Dictionary) -> Error:
		match method:
			"getMe": _complete.call_deferred(client,{"is_bot":true,"username":"fixture_bot"})
			"getChat": _complete.call_deferred(client,{"id":-1001234567890,"type":"supergroup"})
			"getWebhookInfo": _complete.call_deferred(client,{"url":""})
			"getUpdates":
				if int(body.timeout)==0:
					_complete.call_deferred(client,[telegram_message(10,-1001234567890,"/ask@fixture_bot 開始回測")])
				else:
					latest_poll=body
			"sendMessage":
				sent_messages.append(body.duplicate(true))
				_complete.call_deferred(client,{"message_id":sent_messages.size()})
		return OK

	func _complete(client: HTTPRequest, data: Variant) -> void:
		client.request_completed.emit(HTTPRequest.RESULT_SUCCESS,200,PackedStringArray(),
			JSON.stringify({"ok":true,"result":data}).to_utf8_buffer())

	func telegram_message(id: int, target: int, text: String, is_bot := false) -> Dictionary:
		return {"update_id":id,"message":{"chat":{"id":target},"from":{"is_bot":is_bot},
			"text":text,"message_id":id,"message_thread_id":7}}

	func deliver(updates: Array) -> void:
		_complete(receiver,updates)

class StubOptimizer extends PL:
	var requests := 0
	var payloads: Array[Dictionary] = []
	func _send_request(_headers: PackedStringArray, payload: Dictionary) -> void:
		requests+=1
		payloads.append(payload)
		var source := "if UnknownFunction(Close)>0 then Buy next bar at market;"
		if requests==2: source=str(document.source).replace("FastLength(10)","FastLength(4)").replace("SlowLength(30)","SlowLength(8)")
		source="{ 測試候選版 }\n"+source
		var text := JSON.stringify({"source":source,"explanation":"測試候選版"})
		var response := {"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":text}]}]}
		call_deferred("_response",HTTPRequest.RESULT_SUCCESS,200,PackedStringArray(),JSON.stringify(response).to_utf8_buffer())

func _initialize() -> void:
	call_deferred("_test")

func _test() -> void:
	var studio := Studio.instantiate()
	root.add_child(studio)
	if "--test-version-control" in OS.get_cmdline_user_args():
		var test_wb: Control=studio.workbench
		await _wait_until(func(): return not test_wb.busy and test_wb.active>=0)
		var original_count: int=test_wb.documents.size()
		test_wb.new_strategy()
		await _wait_until(func(): return not test_wb.busy and test_wb.documents.size()>original_count)
		if await _verify_strategy_versions(studio): quit(0)
		return
	if "--test-nikkei-roll" in OS.get_cmdline_user_args():
		studio.navigation.current_tab=studio.EXECUTION
		var execution: Control=studio.strategy_execution
		execution._start_replay()
		await _wait_until(func(): return not execution.running,180_000)
		var proof: Dictionary=execution.replay_proof
		if not _expect(proof.get("rollovers",[]).size()==1 and proof.get("segments",[]).size()==2,
			"日經跨季回放失敗："+execution.details.text): return
		if not _expect(proof.rollovers[0].verified_flat and proof.rollovers[0].old_month=="202606"
			and proof.rollovers[0].next_month=="202609" and proof.segments[0].fill_count>0
			and proof.segments[1].fill_count>0,"未驗證舊月歸零與日經策略成交"): return
		print("NIKKEI_ROLL_OK: ", JSON.stringify(proof))
		quit(0)
		return
	if "--test-live-chart" in OS.get_cmdline_user_args():
		await _test_live_chart(studio)
		return
	if "--test-api-profiles" in OS.get_cmdline_user_args():
		_test_api_profiles(studio)
		return
	if "--test-chat-home" in OS.get_cmdline_user_args():
		await _test_chat_home(studio)
		return
	var wb: Control=studio.workbench
	# 這次測試的候選版只透過模擬回覆取得，回測由 Rust 核心實際執行。
	wb.smoke=true
	wb.smoke_stage=-1
	await _wait_until(func(): return not wb.busy and wb.active>=0 and not studio.quote.busy)
	if not _expect(wb.active>=0 and studio.quote.selected_symbol=="REA_SYNTH","工作台或行情未載入"): return
	var optimizer_index: int=wb.optimizer.get_index()
	wb.pages.remove_child(wb.optimizer)
	var old: Control=wb.optimizer
	for button in old.find_children("*","Button",true,false): wb.buttons.erase(button)
	old.queue_free()
	var ai := StubOptimizer.new()
	wb.optimizer=ai
	wb.pages.add_child(ai)
	wb.pages.move_child(ai,optimizer_index)
	ai.build(wb)
	ai.requested.connect(wb._request)
	ai.running_changed.connect(func(_running): wb._set_busy(wb.busy))

	var q: Control=studio.quote
	var handoff: OptionButton
	for button in q.find_children("*","OptionButton",true,false):
		if button.get_item_text(0)=="帶入回測…": handoff=button; break
	if not _expect(handoff!=null,"缺少帶入回測入口"): return
	handoff.item_selected.emit(2)
	if not _expect(studio.navigation.current_tab==studio.STRATEGIES and wb.pages.current_tab==wb.BACKTEST_PAGE and wb.fields.symbol.text=="REA_SYNTH" and wb.fields.database.text==q.database and wb.unit.selected==2 and wb.fields.period.text=="1","行情帶入回測失敗"): return
	wb.editor.text=FileAccess.get_file_as_string(wb.root.path_join("strategies/pl-workbench-example.pl"))
	wb.fields.from.text=""
	wb.fields.to.text="2026/10/26 11:51:00"
	wb.fields.warmup_bars.text="10"
	wb.fields.commission_per_side.text="1"
	var original_source: String=wb.editor.text
	var original_id: String=wb.documents[wb.active].id
	studio.open_page(studio.OPTIMIZE)
	if not _expect(wb.pages.current_tab==3 and not q.visible,"AI 導覽失敗"): return
	OS.unset_environment("OPENAI_API_KEY")
	wb._start_optimization()
	if not _expect(not ai.running and ai.detail.text.contains("金鑰"),"缺少金鑰提示失敗"): return
	ai.api_key.text="stub-test-key"
	ai.goal.text="變更兩個均線長度"
	ai.objective.select(1)
	wb._start_optimization()
	await _wait_until(func(): return not ai.running)
	if not _expect(ai.requests==2 and ai.options.size()==1,"AI 迭代、無效語法重試或候選回測失敗："+ai.detail.text): return
	var candidate: Dictionary=ai.options[0]
	if not _expect(ai.payloads[1].input.contains("UnknownFunction") and candidate.source!=original_source,"第二輪未收到驗證結果"): return
	if not _expect(candidate.receipt.optimization_inputs==ai.baseline.optimization_inputs and candidate.receipt.summary.net_closed!=ai.baseline.summary.net_closed,"基準與候選未依相同行情條件得出不同績效"): return
	if not _expect(ai.best==candidate and ai.detail.text.contains("建議檢視第 2 輪"),"未按最大回撤選出候選版"): return
	ai._draft()
	await _wait_until(func(): return not wb.busy and wb.documents.size()==2)
	if not _expect(wb.documents.size()==2 and wb.documents[0].id==original_id and wb.documents[0].source==original_source and wb.documents[1].source==candidate.source and wb.documents[1].managed_id==null,"候選草稿或原版保留失敗"): return
	studio.open_page(studio.BACKTEST)
	wb._request("view",{"receipt":candidate.receipt,"page":0})
	await _wait_until(func(): return not wb.busy)
	if not _expect(candidate.receipt.market_coverage.symbol=="REA_SYNTH" and candidate.receipt.report_statistics.date_basis=="trading_date" and wb.run_info.text.contains("尚無檢查紀錄") and wb.run_info.text.contains("統計日"),"回測摘要未顯示行情完整性或日期基準："+wb.run_info.text): return
	var report_html: String=FileAccess.get_file_as_string(candidate.receipt.artifacts.html)
	var engine_html: String=FileAccess.get_file_as_string(candidate.receipt.artifacts.engine_html)
	if not _expect(report_html.contains("交易日績效") and report_html.contains("行情完整性") and report_html.contains("2026-10") and report_html.contains("逐筆成交驗收") and report_html.contains("尚無檢查紀錄"),"桌面 HTML 缺少補充績效或行情檢查"): return
	var trade_section := "<h2>交易明細</h2>"
	if not _expect(engine_html.contains(trade_section) and report_html.contains(trade_section) and engine_html.get_slice(trade_section,1)==report_html.get_slice(trade_section,1),"桌面 HTML 抄錄的原始成交明細有差異"): return
	if not _expect(not wb.report_menu.disabled,"回測結果未提供報表與檢測選單"): return
	var report_actions: PopupMenu=wb.report_menu.get_popup()
	report_actions.index_pressed.emit(3)
	wb.robustness_fields["repetitions"].value=2
	wb.robustness_fields["block_bars"].value=10
	wb.robustness_dialog.hide()
	wb.robustness_dialog.confirmed.emit()
	await _wait_until(func(): return not wb.busy)
	var stress: Variant=JSON.parse_string(FileAccess.get_file_as_string(wb.robustness_report_path))
	if not _expect(stress is Dictionary and stress.get("baseline_run_id")==candidate.receipt.run_id and stress.get("scenarios",[]).size()==7,"強健性報告未完成："+wb.status.text): return
	print("ROBUSTNESS_OK: ",wb.robustness_report_path)
	if not _expect(not wb.trade_replay_button.disabled and wb.trades.get_root().get_child_count()>0,"回測交易圖入口未啟用"): return
	wb.trade_replay_button.pressed.emit()
	var replay: Window=wb.trade_replay
	await _wait_until(func(): return replay.loader==null)
	if not _expect(replay.visible and replay.load_error.is_empty() and replay.canvas.times.size()==int(wb.last_view.bar_count),"回測交易圖未讀到完整快照："+replay.load_error): return
	var first_trade: Dictionary=wb.last_view.trades[0]
	var entry := int(first_trade.entry_bar)
	var exit := int(first_trade.exit_bar)
	replay.date_input.text=str(replay.canvas.times[0]).left(10)
	replay.date_button.pressed.emit()
	if not _expect(replay.active_bar==0,"日期跳轉未定位至正確 K 棒"): return
	replay.open_run(candidate.receipt,first_trade)
	if not _expect(replay.active_bar==entry and replay.details.text.contains(first_trade.entry_label),"進場成交未對齊 K 棒"): return
	replay._choose_start()
	replay._pick(entry)
	if not _expect(replay.canvas.limit==entry+1 and (exit==entry or not replay.details.text.contains("淨損益")),"回放顯示了未來成交"): return
	replay.step_button.pressed.emit()
	if not _expect(replay.canvas.limit==entry+2,"無法逐根推進回放"): return
	replay.timer.wait_time=0.01
	replay.play_button.pressed.emit()
	await _wait_until(func(): return replay.canvas.limit>entry+2)
	if not _expect(replay.canvas.limit>entry+2,"自動播放沒有推進 K 棒"): return
	replay.finish_button.pressed.emit()
	if not _expect(replay.canvas.limit==replay.canvas.times.size(),"結束回放後未恢復完整行情"): return
	replay.close_requested.emit()
	print("TRADE_REPLAY_OK: ",replay.run_id)
	studio.navigation.current_tab=studio.EXECUTION
	var execution: Control=studio.strategy_execution
	if not _expect(execution.visible and execution.switch.disabled and execution.strategy_picker.item_count>1,
		"執行搭配頁未載入策略庫或初始狀態錯誤"): return
	var previous: String=wb.action
	var test_strategy: String=execution.strategy_picker.get_item_metadata(1)
	for index in range(1,execution.strategy_picker.item_count):
		var name: String=execution.strategy_picker.get_item_text(index)
		if name.contains("SyntheticOrderLab"):
			test_strategy=execution.strategy_picker.get_item_metadata(index)
			break
	execution._pick(execution.strategy_picker,test_strategy)
	execution.strategy_picker.item_selected.emit(execution.strategy_picker.selected)
	execution.profiles.add_item("測試交易設定")
	execution.profiles.set_item_metadata(execution.profiles.item_count-1,"fixture")
	execution._pick(execution.profiles,"fixture")
	execution.period_minutes.value=5
	execution.warmup_bars.value=23
	execution._save_combination()
	var first_id: String=execution.selected_id
	if not _expect(not first_id.is_empty() and execution.configurations[0].period_minutes==5 and execution.configurations[0].warmup_bars==23,
		"策略與商品設定未儲存在同一搭配"): return
	execution._add_combination()
	execution._pick(execution.strategy_picker,test_strategy)
	execution.strategy_picker.item_selected.emit(execution.strategy_picker.selected)
	execution._pick(execution.product_picker,"MXF")
	execution.product_picker.item_selected.emit(execution.product_picker.selected)
	execution.period_minutes.value=10
	execution._save_combination()
	if not _expect(execution.configurations.size()==2 and execution.selected_id!=first_id and execution.configurations[1].product=="MXF",
		"同策略用於不同商品應保留獨立列"): return
	var list_root: TreeItem=execution.combinations.get_root()
	if not _expect(list_root.get_child(0).get_text(3)=="5 分 K" and list_root.get_child(1).get_text(3)=="10 分 K",
		"搭配列表未顯示各自週期"): return
	await process_frame
	await process_frame
	var handle: Control=execution.split.get_drag_area_control()
	if not _expect(handle!=null and handle.size.x>=20 and handle.mouse_default_cursor_shape==Control.CURSOR_HSIZE,
		"搭配清單缺少可拖曳的分隔線"): return
	execution._show_combination(execution.configurations[0])
	if not _expect(not execution.switch.disabled and execution._profile()=="fixture" and execution._product()=="TXF" and execution.warmup_bars.value==23,
		"切換搭配沒有帶回獨立的商品、帳戶與策略設定"): return
	execution.active_run=first_id
	execution._set_run_state("已啟用")
	if not _expect(execution.combinations.get_root().get_child(0).get_text(0)=="已啟用"
		and execution.combinations.get_root().get_child(1).get_text(0)=="已關閉","啟用狀態應依搭配獨立顯示"): return
	execution.active_run=""
	execution.switch.button_pressed=true
	if not _expect(execution.confirmation.visible and not execution.running and wb.action==previous and execution.pending.period_minutes==5
		and execution.pending.warmup_bars==23,"啟用應傳入搭配週期與暖機並等待正式交易確認"): return
	execution.confirmation.canceled.emit()
	execution.confirmation.hide()
	if not _expect(execution.pending.is_empty() and not execution.switch.button_pressed,"取消確認後仍有待啟動策略"): return
	execution._open_foreign_order()
	var foreign: Control=execution.foreign_order
	if not _expect(not foreign._resolve_nikkei().is_empty(),"日經連續商品未能解析交易對應"): return
	foreign.connect_button.pressed.emit()
	await _wait_until(func(): return foreign.broker_ready and foreign.recovered)
	if not _expect(foreign.broker_ready and foreign.mode.selected==0,"海外模擬連線未就緒"): return
	foreign.price.text="123.25"
	foreign.mode.selected=1
	foreign.send_button.pressed.emit()
	if not _expect(foreign.confirmation.visible and foreign.awaiting.is_empty(),"海外正式委託未等待逐筆確認"): return
	foreign.confirmation.canceled.emit()
	foreign.confirmation.hide()
	foreign.mode.selected=0
	foreign.send_button.pressed.emit()
	await _wait_until(func(): return foreign.status.text.contains("模擬委託回報"))
	if not _expect(foreign.status.text.contains("模擬委託回報"),"海外委託未收到模擬回報："+foreign.status.text): return
	foreign.connect_button.pressed.emit()
	foreign.dialog.hide()
	print("FOREIGN_ORDER_OK: 海外模擬連線、送單與回報")
	execution.replay_button.pressed.emit()
	if not _expect(execution.replay_dialog.visible,"225M 海外自動模擬入口未開啟"): return
	execution.replay_quarterly.button_pressed=false
	execution.replay_from.text="2025-05-27"
	execution.replay_to.text="2025-06-26"
	execution.replay_dialog.hide()
	execution.replay_dialog.confirmed.emit()
	await _wait_until(func(): return not execution.running)
	var replay_proof: Dictionary=execution.replay_proof
	if not _expect(not replay_proof.is_empty() and replay_proof.fill_count>0 and replay_proof.position_checks==replay_proof.fill_count+2 and replay_proof.trade_validation.status=="passed","225M 海外自動模擬或部位對帳失敗："+execution.details.text): return
	for label in ["N225LE","N225SE","N225L0Stop","N225S0Stop","N225L0Goal","N225S0Goal","N225LXTime"]:
		if not _expect(replay_proof.fills_by_label.has(label),"225M 模擬未覆蓋 "+label): return
	print("FOREIGN_STRATEGY_REPLAY_OK: ",JSON.stringify(replay_proof))
	for index in studio.navigation.tab_count:
		if not _expect(studio.navigation.get_tab_title(index)!="交易","手動交易分頁仍存在"): return
	execution.reports_requested.emit("fixture")
	if not _expect(studio.navigation.current_tab==studio.REPORTS and studio.trade_reports.visible,"策略交易回報入口未開啟"): return
	print("STRATEGY_EXECUTION_OK: 策略清單、連續商品與正式交易確認")
	var logic_tab: int=wb.active
	if not await _verify_strategy_versions(studio): return
	studio.open_page(studio.CODE)
	wb.doc_tabs.current_tab=logic_tab
	await _wait_until(func(): return not wb.busy and wb.active==logic_tab)
	wb._document_action("strategy_logic")
	var logic: Array=await wb.operation_finished
	if not _expect(logic[0]=="strategy_logic" and logic[2].is_empty() and logic[1].statement_count>0 and FileAccess.file_exists(logic[1].report_path),"策略流程圖未完成"): return
	if not _expect(FileAccess.file_exists(logic[1].excalidraw_path),"可編輯策略流程圖未匯出"): return
	print("STRATEGY_LOGIC_OK: ",logic[1].report_path)
	print("STUDIO_OK: ",JSON.stringify({"baseline":ai.baseline.summary,"candidate":candidate.receipt.summary,"baseline_run":ai.baseline.run_id,"candidate_run":candidate.receipt.run_id}))
	quit(0)

func _verify_strategy_versions(studio: Control) -> bool:
	var wb: Control=studio.workbench
	var original_tab: int=wb.active
	var initial_count: int=wb.documents.size()
	var started_source: String=wb.editor.text
	studio.navigation.current_tab=studio.STRATEGIES
	wb.workspace_tools.get_popup().index_pressed.emit(5)
	await _wait_until(func(): return not wb.busy and wb.version_status.text.contains("個本機版本"))
	if not _expect(studio.navigation.get_tab_title(studio.STRATEGIES)=="策略工作區" and wb.pages.current_tab==wb.VERSION_PAGE and wb.version_list.is_visible_in_tree(),"從策略工具選單開啟版本控制失敗"): return false
	wb.version_message.text="初始版本"
	wb._version_request("commit")
	await _wait_until(func(): return not wb.busy and wb.version_list.item_count==1)
	var first_version: String=wb.version_list.get_item_metadata(0)
	wb.inputs.text=JSON.stringify({"VersionTestLength":12})
	wb.version_message.text="修改參數"
	wb._version_request("commit")
	await _wait_until(func(): return not wb.busy and wb.version_list.item_count==2)
	if not _expect(wb.version_source.text==started_source and JSON.parse_string(wb.version_parameters.text).parameters.inputs.VersionTestLength==12 and not wb.version_source.editable,"版本程式碼或回測設定預覽不符"): return false
	wb.version_list.select(1)
	wb._version_request("draft")
	await _wait_until(func(): return not wb.busy and wb.documents.size()==initial_count+1)
	if not _expect(wb.documents.size()==initial_count+1 and wb.documents[wb.active].source==started_source and wb.documents[wb.active].task.parameters.inputs.get("VersionTestLength")==null and wb.documents[original_tab].task.parameters.inputs.get("VersionTestLength")==12,"策略版本還原草稿未保留原分頁或參數"): return false
	wb.doc_tabs.current_tab=original_tab
	await _wait_until(func(): return not wb.busy and wb.active==original_tab)
	studio.open_page(studio.VERSIONS)
	await _wait_until(func(): return not wb.busy and wb.version_list.item_count==2)
	wb.version_list.select(1)
	wb._confirm_version_apply()
	wb.confirm.hide()
	wb.confirm.confirmed.emit()
	await _wait_until(func(): return not wb.busy and not wb.version_pending_refresh)
	if not _expect(wb.documents.size()==initial_count+1 and wb.documents[original_tab].task.parameters.inputs.get("VersionTestLength")==null,"策略版本未套用至原分頁"): return false
	print("STRATEGY_VERSION_OK: ",first_version)
	var managed_entry: Dictionary={}
	for catalog_entry in wb.catalog:
		if catalog_entry.id=="REA_SyntheticOrderLab": managed_entry=catalog_entry; break
	if not _expect(not managed_entry.is_empty(),"找不到 UI 與 MCP 共用測試策略"): return false
	wb._request("open",{"entry":managed_entry})
	await _wait_until(func(): return not wb.busy and wb.version_status.text.contains("0 個本機版本"))
	if not _expect(studio.navigation.current_tab==studio.STRATEGIES and wb.pages.current_tab==wb.VERSION_PAGE and wb.documents[wb.active].id==managed_entry.id,"從左側策略庫選擇後未留在版本控制頁"): return false
	wb.version_message.text="從桌面提交供 MCP 查詢"
	wb._version_request("commit")
	await _wait_until(func(): return not wb.busy and wb.version_list.item_count==1)
	if not _expect(wb.version_list.item_count==1,"桌面提交共用版本失敗"): return false
	print("STRATEGY_VERSION_SHARED: ",wb.version_list.get_item_metadata(0))
	return true

func _test_chat_home(studio: Control) -> void:
	var wb: Control = studio.workbench
	var chat: Control = studio.chat
	if not _expect(studio.navigation.get_tab_title(0)=="對話首頁", "對話首頁未排在第一個頁籤"): return
	if not _expect(chat.visible and not wb.visible and studio.navigation.current_tab == studio.HOME, "程式未從對話首頁啟動"): return
	wb.smoke=true
	wb.smoke_stage=-1
	await _wait_until(func(): return not wb.busy and wb.active >= 0)
	await _chat_send(chat,"查詢 REA_SYNTH 行情來源")
	if not _expect(_chat_reply(chat).contains("1 分 K · 171 筆"), "對話未讀到實際行情來源"): return
	var quote: Control = studio.quote
	if not _expect(wb.select_market(quote.database,quote.current,1), "無法指定回測測試行情"): return
	wb.editor.text=FileAccess.get_file_as_string(wb.root.path_join("strategies/pl-workbench-example.pl"))
	wb.fields.from.text=""
	wb.fields.to.text="2026/10/26 11:51:00"
	wb.fields.warmup_bars.text="10"
	wb.fields.commission_per_side.text="1"
	wb.excel.button_pressed=false
	await _chat_send(chat,"檢查目前策略語法")
	if not _expect(_chat_reply(chat).contains("語法"), "對話未回傳語法檢查結果"): return
	var test_document: Dictionary = wb.documents[wb.active]
	var original_catalog: Array = wb.catalog
	wb.catalog = [
		{"id":"fixture-225m-a","name":"225M 測試版本 A","kind":"strategy","source":test_document.get("origin")},
		{"id":"fixture-225m-b","name":"225M 測試版本 B","kind":"strategy","source":"unused.pl"},
		{"id":"fixture-function","name":"225M 測試函式","kind":"function","source":"unused.eld"}
	]
	if not _expect(chat.find_children("*","CheckBox",true,false).is_empty(),"對話首頁仍有 AI 勾選開關"): return
	await _chat_send(chat,"有幾個日經策略")
	var strategy_list := _chat_reply(chat)
	if not _expect("有 2 個" in strategy_list and "225M 測試版本 A" in strategy_list and "225M 測試版本 B" in strategy_list and "測試函式" not in strategy_list, "AI 啟用時應直接依策略庫列出日經策略"): return
	for prompt in ["有幾隻策略","目前總共有幾隻策略"]:
		await _chat_send(chat,prompt)
		if not _expect("策略庫共有 2 個" in _chat_reply(chat),"應由本機策略庫回答策略總數"): return
	await _chat_send(chat,"有多少個不存在的策略？")
	if not _expect("有 0 個" in _chat_reply(chat), "查無策略時應回覆零筆"): return
	await _chat_send(chat,"回測日經策略")
	var choices: VBoxContainer = chat.transcript.get_child(-1).get_child(0)
	if not _expect(_chat_reply(chat).contains("請選擇") and choices.get_child_count() == 4 and test_document.get("result") == null, "日經策略應列出兩個候選並等候選擇"): return
	if not _expect(choices.get_child(2).text == "回測 225M 測試版本 A" and choices.get_child(3).text == "回測 225M 測試版本 B", "回測候選名稱有誤"): return
	choices.get_child(2).pressed.emit()
	await _wait_until(func(): return chat.pending.is_empty())
	var receipt: Dictionary=wb.documents[wb.active].get("result",{})
	if not _expect(receipt.get("status")=="completed" and _chat_reply(chat).contains(receipt.run_id) and _chat_reply(chat).contains("已平倉 4 筆"), "提示詞未完成實際回測或回傳摘要"): return
	await _chat_send(chat,"回測 225M 測試版本 A")
	var named_receipt: Dictionary=wb.documents[wb.active].get("result",{})
	if not _expect(named_receipt.get("status")=="completed" and named_receipt.run_id != receipt.run_id, "完整策略名稱未觸發指定策略回測"): return
	receipt = named_receipt
	await _chat_send(chat,"回測不存在的測試策略")
	if not _expect(_chat_reply(chat).contains("策略庫找不到") and wb.documents[wb.active].result.run_id == receipt.run_id, "找不到策略時應提示選取且不回測目前策略"): return
	wb.catalog = original_catalog
	if not await _test_chart_strategy(studio,receipt): return
	await _chat_send(chat,"摘要最近回測報表")
	if not _expect(_chat_reply(chat).contains(receipt.run_id), "對話未沿用目前策略的報表"): return
	var existing: String = wb.editor.text
	var proposed := "{ 首次評估時掛次棒市價單 }\nif CurrentBar = 1 then Buy next bar at market;"
	var response := {"status":"completed","output":[{"type":"message","content":[{"type":"output_text",
		"text":JSON.stringify({"action":"draft","source":proposed})}]}]}
	chat.model_document_id=wb.documents[wb.active].id
	chat.pending="model"
	chat._model_done(HTTPRequest.RESULT_SUCCESS,200,PackedStringArray(),JSON.stringify(response).to_utf8_buffer())
	if not _expect(chat.pending.is_empty() and _chat_reply(chat).contains("需要可驗收") and wb.editor.text==existing, "未確認驗收時仍接受草稿"): return
	chat.model_context=chat._research_context()
	chat.model_prompt="製作測試策略"
	chat.pending="model"
	response.output[0].content[0].text=JSON.stringify({"action":"clarify","message":"模型自行提出的問題。",
		"questions":[{"question":"模型自行提出的題目","options":["模型選項甲","模型選項乙"]}]})
	chat._model_done(HTTPRequest.RESULT_SUCCESS,200,PackedStringArray(),JSON.stringify(response).to_utf8_buffer())
	await _wait_until(func(): return chat.pending.is_empty())
	if not _expect(is_instance_valid(chat.review_form),"MCP 沒有產生需求表單"): return
	var form: VBoxContainer=chat.review_form
	var picker: OptionButton=form.get_child(1)
	var custom: LineEdit=form.get_child(2)
	var fields := [{"key":"market","picker":picker,"custom":custom}]
	if not _expect(form.get_child(0).text.contains("要使用哪個商品") and picker.get_item_text(1).begins_with("建議 · 沿用 REA_SYNTH")
		and picker.get_item_text(3)=="自行描述" and picker.get_item_metadata(3).free_text and not custom.visible,"工作台未呈現 MCP 的原始問題與選項"): return
	picker.select(3)
	picker.item_selected.emit(3)
	if not _expect(custom.visible and chat._review_answers(fields).is_empty(),"自行描述未展開或允許空白答案"): return
	custom.text="MXF，5 分 K"
	if not _expect(chat._review_answers(fields).get("market")==custom.text,"自行描述未納入答案"): return
	if not _expect(_chat_reply(chat).contains("MCP") and not chat._approve_research() and wb.editor.text==existing,"仍有問題時不應接受確認"): return
	chat.model_prompt="同意，進場測試草稿採次棒市價"
	chat.pending="model"
	response.output[0].content[0].text=JSON.stringify({"action":"research_plan","research_action":"draft",
		"acceptance":{"market":"沿用目前設定","trade_rules":"次棒市價進場","backtest":"沿用測試設定",
			"pass_criteria":"檢查次棒成交","evidence":"交付 PL 草稿"}})
	chat._model_done(HTTPRequest.RESULT_SUCCESS,200,PackedStringArray(),JSON.stringify(response).to_utf8_buffer())
	await _wait_until(func(): return chat.pending.is_empty())
	if not _expect(_chat_reply(chat).contains("確認驗收") and chat._approve_research(),"未建立可確認的驗收清單"): return
	response.output[0].content[0].text=JSON.stringify({"action":"draft","source":proposed})
	chat.pending="model"
	chat._model_done(HTTPRequest.RESULT_SUCCESS,200,PackedStringArray(),JSON.stringify(response).to_utf8_buffer())
	await _wait_until(func(): return chat.pending.is_empty())
	if not _expect(wb.documents.size()==2 and wb.editor.text==proposed and wb.documents[0].source==existing, "確認後未另開 PL 草稿或改寫了原策略"): return
	await _chat_send(chat,"開啟 API 設定")
	if not _expect(studio.settings_sections.current_tab==studio.SETTINGS_BROKER and studio.api_profiles.is_visible_in_tree() and not chat.visible, "對話未開啟 API 設定"): return
	studio.navigation.current_tab=studio.HOME
	await _chat_send(chat,"開啟 Telegram 設定")
	var settings: Control=studio.settings
	if not _expect(studio.settings_sections.current_tab==studio.SETTINGS_TELEGRAM and settings.telegram_section.is_visible_in_tree() and not settings.chat_id.editable and not settings.model.editable, "設定頁未循唯讀流程開啟"): return
	settings.edit_button.pressed.emit()
	settings.model.text="fixture-openai-model"
	settings.chat_id.text="-1001234567890"
	settings.credentials.ai_key.field.text="fixture-openai-key-224c"
	settings.credentials.telegram_bot_token.field.text="987654321:fixture-Telegram-224c"
	settings.save_button.pressed.emit()
	if not _expect(settings.status.text=="已儲存工作台設定。" and not settings.model.editable and settings.chat_id.text=="-1001234567890", "工作台設定保存失敗"): return
	if not _expect(settings.credentials.ai_key.field.placeholder_text=="******" and settings.credentials.telegram_bot_token.field.text.is_empty(), "工作台設定未遮罩"): return
	var connection: Dictionary=wb.optimizer.connection_settings()
	if not _expect(connection.model=="fixture-openai-model" and connection.key=="fixture-openai-key-224c", "AI 優化與對話共用的連線設定錯誤"): return
	settings.credentials.telegram_bot_token.toggle.pressed.emit()
	if not _expect(settings.credentials.telegram_bot_token.display.text=="987654321:fixture-Telegram-224c", "Telegram Bot Token 無法讀回"): return
	studio.navigation.current_tab=studio.HOME
	if not _expect(settings.credentials.telegram_bot_token.display.text.is_empty(), "離開設定時未隱藏令牌"): return
	studio.navigation.current_tab=studio.SETTINGS
	settings.edit_button.pressed.emit()
	settings.chat_id.text="@fixture_channel"
	settings.save_button.pressed.emit()
	if not _expect(settings.chat_id.text=="@fixture_channel" and settings.ai_config("openai").key=="fixture-openai-key-224c" and settings.credentials.telegram_bot_token.stored, "變更 Chat ID 時未保留既有金鑰或令牌"): return
	settings.edit_button.pressed.emit()
	settings.provider.select(2)
	settings.provider.item_selected.emit(2)
	settings.model.text="fixture-gemini-default"
	settings.model_choices.text="fixture-gemini-extra"
	settings.credentials.ai_key.field.text="fixture-gemini-key-224c"
	settings.provider.select(3)
	settings.provider.item_selected.emit(3)
	if not _expect(settings.displayed_source=="grok" and settings.model_presets.get_item_metadata(1)=="grok-4.7", "未顯示 Grok 對應模型"): return
	settings.model_presets.select(2)
	settings.model_presets.item_selected.emit(2)
	settings.credentials.ai_key.field.text="fixture-grok-key-224c"
	settings.provider.select(4)
	settings.provider.item_selected.emit(4)
	if not _expect(settings.displayed_source=="mixroute" and settings.model_presets.get_item_metadata(1)=="gemini-3.6-flash", "未顯示 MixRoute 對應模型"): return
	var sol_models := ["gpt-6.1-sol", "gpt-6.2-sol"]
	var presets := range(settings.model_presets.item_count).map(settings.model_presets.get_item_metadata)
	if not _expect(sol_models.all(func(name): return name in presets), "MixRoute 設定缺少 GPT Sol 模型"): return
	settings.model_presets.select(2)
	settings.model_presets.item_selected.emit(2)
	settings.credentials.ai_key.field.text="fixture-mixroute-key-224c"
	settings.provider.select(1)
	settings.provider.item_selected.emit(1)
	settings.model.text="fixture-claude-default"
	settings.credentials.ai_key.field.text="fixture-anthropic-key-224c"
	settings.save_button.pressed.emit()
	var default_connection: Dictionary=wb.optimizer.connection_settings()
	if not _expect(default_connection.provider=="anthropic" and default_connection.model=="fixture-claude-default" and default_connection.key=="fixture-anthropic-key-224c", "預設 AI 來源或金鑰未套用"): return
	var grok: Dictionary=wb.optimizer.connection_settings("grok")
	var mixroute: Dictionary=wb.optimizer.connection_settings("mixroute")
	if not _expect(grok.model=="grok-4.6" and grok.key=="fixture-grok-key-224c" and mixroute.model=="claude-sonnet-5-5" and mixroute.key=="fixture-mixroute-key-224c", "新增來源未保留自己的模型與金鑰"): return
	var default_before_shortcut: String=settings.saved_provider
	settings.open_provider_key("mixroute")
	if not _expect(settings.editing and settings.displayed_source=="mixroute" and settings.credentials.ai_key.field.secret,"MixRoute 設定入口未啟用隱碼編輯"): return
	settings.credentials.ai_key.field.text="fixture-mixroute-key-224c"
	settings._save()
	if not _expect(settings.saved_provider==default_before_shortcut and settings.saved_sources.mixroute.key_set,"MixRoute 金鑰未儲存或變更了預設來源"): return
	var selected := -1
	var found_sources := {}
	var mixroute_choices := {}
	for index in chat.model_picker.item_count:
		var data: Dictionary=chat.model_picker.get_item_metadata(index)
		if data.has("provider"):
			found_sources[data.provider]=true
		if data.get("provider")=="mixroute":
			mixroute_choices[data.model]=index
		if data.get("model")=="fixture-gemini-extra":
			selected=index
	if not _expect(selected>=0 and found_sources.size()==5,"對話模型選單未載入完整來源"): return
	for name in sol_models:
		if not _expect(mixroute_choices.has(name), "對話模型選單缺少 MixRoute " + name): return
		chat.model_picker.select(mixroute_choices[name])
		var sol: Dictionary = chat._model_selection()
		var connection_sol: Dictionary = wb.optimizer.connection_settings(sol.provider, sol.model)
		if not _expect(connection_sol.model==name and connection_sol.key==mixroute.key, "GPT Sol 選項未使用 MixRoute 連線設定"): return
	chat.model_picker.select(selected)
	var choice: Dictionary=chat._model_selection()
	var gemini: Dictionary=wb.optimizer.connection_settings(choice.provider,choice.model)
	if not _expect(gemini.model=="fixture-gemini-extra" and gemini.key=="fixture-gemini-key-224c" and settings.ai_config("openai").key=="fixture-openai-key-224c", "對話模型切換時混用了來源金鑰"): return
	var template := {"model":gemini.model,"input":"測試","instructions":"回傳 JSON","max_output_tokens":500,
		"text":{"format":{"name":"test_schema","schema":{"type":"object"}}}}
	if not _expect(AI.endpoint(gemini.provider,gemini.model).begins_with("https://generativelanguage.googleapis.com/") and AI.headers(gemini.provider,gemini.key)[1].begins_with("x-goog-api-key:") and AI.payload_for(gemini.provider,template).generationConfig.responseFormat.text.schema.type=="object", "Gemini API 請求格式錯誤"): return
	var gemini_reply := {"candidates":[{"finishReason":"STOP","content":{"parts":[{"thought":true,"text":"略"},{"text":"{\"action\":\"reply\"}"}]}}]}
	var claude_reply := {"stop_reason":"end_turn","content":[{"type":"text","text":"{\"action\":\"reply\"}"}]}
	if not _expect(AI.read_json("gemini",JSON.stringify(gemini_reply).to_utf8_buffer()).action=="reply" and AI.read_json("anthropic",JSON.stringify(claude_reply).to_utf8_buffer()).action=="reply" and AI.payload_for("anthropic",template).output_config.format.schema.type=="object", "Claude 或 Gemini 結構化回覆無法解析"): return
	var compatible_reply := {"choices":[{"finish_reason":"stop","message":{"content":"{\"action\":\"reply\"}"}}]}
	var endpoints := {"grok":"https://api.x.ai/v1/chat/completions","mixroute":"https://api.mixroute.ai/v1/chat/completions"}
	for source in endpoints:
		var connection_data: Dictionary=wb.optimizer.connection_settings(source)
		template.model=connection_data.model
		var payload := AI.payload_for(source,template)
		if not _expect(AI.endpoint(source,connection_data.model)==endpoints[source] and AI.headers(source,connection_data.key)[1]=="Authorization: Bearer "+connection_data.key and payload.model==connection_data.model and payload.response_format.json_schema.name=="test_schema" and AI.read_json(source,JSON.stringify(compatible_reply).to_utf8_buffer()).action=="reply", "Grok 或 MixRoute 相容 API 轉接錯誤"): return
	wb._activate(0)
	await _wait_until(func(): return not wb.busy)
	var bot := StubTelegram.new()
	studio.add_child(bot)
	bot.config_loader=settings.telegram_config
	bot.state_changed.connect(func(message: String,_active: bool): bot.last_state=message)
	bot.prompt_received.connect(chat.submit_telegram)
	bot.start(settings.telegram_config(),true)
	await _wait_until(func(): return bot.latest_poll.get("offset")==11 and bot.last_state.contains("接受測試訊息"))
	if not _expect(bot.sent_messages.size()==1 and bot.sent_messages[0].chat_id==-1001234567890 and not wb.busy, "Telegram 啟動測試或舊訊息跳過失敗"): return
	bot.deliver([
		bot.telegram_message(11,-100111,"/ask@fixture_bot 開始回測"),
		bot.telegram_message(12,-1001234567890,"/ask@other_bot 開始回測"),
		bot.telegram_message(13,-1001234567890,"/ask@fixture_bot 開始回測",true),
		bot.telegram_message(14,-1001234567890,"/ping@fixture_bot")])
	await _wait_until(func(): return bot.last_state.contains("測試通過") and bot.latest_poll.get("offset")==15)
	if not _expect(bot.sent_messages.size()==2 and bot.sent_messages[1].text.contains("Pong") and bot.sent_messages[1].message_thread_id==7 and not wb.busy, "Telegram 收發或 Chat ID 比對失敗"): return
	bot.deliver([
		bot.telegram_message(15,-1001234567890,"/ask@fixture_bot 開始回測"),
		bot.telegram_message(15,-1001234567890,"/ask@fixture_bot 開始回測")])
	await _wait_until(func(): return bot.sent_messages.size()>=4 and chat.pending.is_empty())
	var remote_result: Dictionary=wb.documents[wb.active].result
	if not _expect(bot.sent_messages.size()==4 and bot.sent_messages[3].text.contains(remote_result.run_id) and bot.sent_messages[3].text.contains("已平倉 4 筆") and remote_result.run_id!=receipt.run_id, "Telegram 提示詞未回傳實際新回測摘要或重複執行"): return
	if not _expect(bot.sent_messages[3].reply_parameters.message_id==15 and bot.sent_messages[3].chat_id==-1001234567890 and bot.sent_messages[3].message_thread_id==7, "回測摘要未回到同一對話與主題"): return
	bot.stop()
	studio.navigation.current_tab=studio.LIVE_CHART
	var count: int=wb.documents.size()
	studio.live_chart.strategy_panel.new_button.pressed.emit()
	await _wait_until(func(): return not wb.busy and wb.documents.size()==count+1)
	if not _expect(studio.navigation.current_tab==studio.STRATEGIES and wb.pages.current_tab==wb.EDIT_PAGE and wb.documents.size()==count+1,"圖表未透過編輯器新增策略草稿"): return
	if not _expect(wb.select_market(quote.database,quote.current,1),"無法設定同棒驗收測試行情"): return
	wb.editor.text="if CurrentBar = 1 then Buy (\"SameEntry\") next bar at market; if CurrentBar = 2 then Sell (\"SameExit\") this bar on close;"
	wb.fields.from.text=""
	wb.fields.to.text="2026/10/26 11:51:00"
	wb.fields.warmup_bars.text="10"
	wb.excel.button_pressed=false
	wb._run()
	await _wait_until(func(): return not wb.busy)
	var audit: Dictionary=wb.last_view.get("receipt",{}).get("trade_validation",{})
	if not _expect(audit.get("status")=="rejected" and audit.get("violation_count")==1 and wb.trade_count==1
		and wb.run_info.text.contains("驗收未通過") and wb.output.get_parsed_text().contains("SameEntry")
		and wb.output.get_parsed_text().contains("SameExit"),"桌面未保留同棒成交並顯示違規名稱與驗收狀態"): return
	print("TELEGRAM_OK: ",remote_result.run_id)
	print("CHAT_HOME_OK: ",receipt.run_id)
	quit(0)

func _test_chart_strategy(studio: Control, receipt: Dictionary) -> bool:
	var overlay=preload("res://chart_strategy_overlay.gd")
	var candle_times:=PackedStringArray(["2026-11-19T08:50:00","2026-11-19T09:05:00"])
	var overview: Dictionary=overlay.match_fills([
		{"datetime":"2026-11-19T08:41:00","is_entry":true},
		{"datetime":"2026-11-19T08:50:00","is_entry":false},
		{"datetime":"2026-11-19T08:51:00","is_entry":true},
		{"datetime":"2026-11-19T08:56:00","is_entry":true}
	],candle_times,PackedFloat64Array(),{"kind":"minute","amount":5,"overview_group":2,"bars":[
		[candle_times[0],0,0,0,0,0,false,"2026-11-19T08:45:00"],
		[candle_times[1],0,0,0,0,0,false,"2026-11-19T09:00:00"]
	]})
	if not _expect(overview.get(0,[]).size()==2 and overview.get(1,[]).size()==1,"全期間概覽漏標進出場或將休盤成交歸入 K 棒"): return false
	var wb: Control=studio.workbench
	var doc: Dictionary=wb.documents[wb.active]
	var original_catalog: Array=wb.catalog
	var original_id: Variant=doc.get("managed_id")
	var out: String=OS.get_environment("STRATEGY_HISTORICAL_MARKET_DB").get_base_dir()
	var task: String=out.path_join("chart-strategy-task.json")
	var task_file:=FileAccess.open(task,FileAccess.WRITE)
	if task_file==null: return _expect(false,"無法建立測試策略設定")
	var saved_task: Dictionary=doc.task.duplicate(true)
	saved_task.market={"symbol":"TXF1","unit":"minutes","period":10}
	saved_task.parameters.point_value=200
	saved_task.parameters.tick_dataset_id=999
	var saved_task_text := JSON.stringify(saved_task)
	task_file.store_string(saved_task_text)
	task_file.close()
	var entry := {"id":"fixture-chart-pl","name":"圖表測試策略","kind":"strategy","source":receipt.artifacts.source,"task":task,"managed":false}
	doc.managed_id=entry.id
	wb.catalog=[entry]
	studio.navigation.current_tab=studio.LIVE_CHART
	var view: Control=studio.live_chart
	var panel: Control=view.strategy_panel
	await _wait_until(func(): return view.graph.limit==171)
	if not _expect(view.graph.limit==171 and panel.picker.item_count==2,"圖表策略選單或歷史 K 線未載入"): return false
	panel.picker.select(1)
	panel.picker.item_selected.emit(1)
	await _wait_until(func(): return panel.report_loaded and not view.graph.fills.is_empty())
	if not _expect(panel.report_loaded and panel.message.text.contains(receipt.run_id) and view.graph.fills.size()==8,"選擇策略後應顯示既有回測進出場"): return false
	var indices: Array=view.graph.fills.keys()
	indices.sort()
	view._inspect(indices[0])
	if not _expect(view.graph.fills[indices[0]][0].is_entry and view.quote.tooltip_text.contains("回測 進場"),"策略進場標記或明細錯誤"): return false
	view._inspect(indices[1])
	if not _expect(not view.graph.fills[indices[1]][0].is_entry and view.quote.tooltip_text.contains("回測 出場"),"策略出場標記或明細錯誤"): return false
	view.unit.select(3)
	view.unit.item_selected.emit(3)
	view.period.value=5
	await _wait_until(func(): return view.title.text.contains("5 分 K") and not view.graph.fills.is_empty())
	var mapped := 0
	for marks: Array in view.graph.fills.values(): mapped+=marks.size()
	if not _expect(mapped==8,"切換 5 分 K 後應重新對齊全部策略進出場"): return false
	view.end_field.text="2026/10/26"
	view._apply_dates()
	panel.run_button.pressed.emit()
	await _wait_until(func(): return panel.run_stage.is_empty() and panel.report_loaded and not panel.message.text.contains(receipt.run_id))
	var result: Dictionary=panel.selected_receipt
	if not _expect(panel.report_loaded and result.get("run_id")==panel.message.text.split(" ")[1] and result.get("dataset",{}).get("symbol")=="REA_SYNTH","圖表未使用目前商品回測："+panel.message.text): return false
	var run_task: Variant=JSON.parse_string(FileAccess.get_file_as_string(result.artifacts.task))
	if not _expect(run_task is Dictionary and run_task.market.period==5 and run_task.parameters.interval_minutes==5
		and run_task.parameters.point_value==1 and run_task.parameters.tick_dataset_id==null
		and run_task.parameters.from=="" and run_task.parameters.to=="2026-10-26T23:59:59"
		and FileAccess.get_file_as_string(task)==saved_task_text,"圖表回測未採用自身週期、日期與商品規格，或覆寫了策略設定"): return false
	panel.picker.select(0)
	panel.picker.item_selected.emit(0)
	if not _expect(view.graph.fills.is_empty(),"移除策略後標記仍留在圖表"): return false
	doc.managed_id=original_id
	wb.catalog=original_catalog
	studio.navigation.current_tab=studio.HOME
	print("CHART_STRATEGY_OK")
	return true

func _chat_send(chat: Control, text: String) -> void:
	chat.composer.text=text
	chat.send.pressed.emit()
	await _wait_until(func(): return chat.pending.is_empty())

func _chat_reply(chat: Control) -> String:
	return chat.transcript.get_child(-1).get_child(0).get_child(1).text

func _test_api_profiles(studio: Control) -> void:
	studio.open_settings(studio.SETTINGS_BROKER)
	var page: Control = studio.api_profiles
	if not _expect(page != null and page.is_visible_in_tree() and not studio.workbench.visible, "API 連線設定導覽失敗"): return
	var first: TreeItem = page.profile_list.get_root().get_first_child()
	if not _expect(first != null and first.get_text(0) == "gateway-test" and first.get_text(2) == "行情" and first.get_next() == null, "行情列表未讀到 MCP 儲存的設定"): return
	var password: LineEdit = page.secrets.password.field
	if not _expect(page.fields.port.text == "5432" and password.secret and password.text.is_empty() and password.placeholder_text == "******", "設定值或憑證遮罩錯誤"): return
	if not _expect(page.edit_button.visible and not page.fields.port.editable and not password.editable and page.save_button.disabled and page.secrets.password.clear.disabled, "既有設定應先以唯讀開啟"): return
	page.secrets.password.toggle.pressed.emit()
	if not _expect(page.secrets.password.display.text.begins_with("fixture-secret-") and page.secrets.password.display.visible and not page.secrets.password.display.editable and password.text.is_empty(), "無法唯讀顯示已儲存密碼"): return
	page.secrets.password.toggle.pressed.emit()
	if not _expect(password.secret and password.visible and page.secrets.password.display.text.is_empty(), "隱藏後仍保留明文"): return
	if not _expect(not page.certificate.visible and not page.extras.visible and not page.fields.connection.get_parent().visible, "凱基行情欄位應隱藏憑證及自訂設定"): return
	page.edit_button.pressed.emit()
	if not _expect(page.fields.port.editable and password.editable and password.placeholder_text == "******" and not page.save_button.disabled and page.cancel_button.visible, "修改按鈕未啟用編輯"): return
	page.fields.port.text = "5433"
	page.save_button.pressed.emit()
	if not _expect(page.status.text == "已儲存 API 設定。" and page.fields.port.text == "5433" and not page.fields.port.editable and password.placeholder_text == "******", "UI 儲存或返回閱讀模式失敗："+page.status.text): return
	page.filter.select(2)
	page.filter.item_selected.emit(2)
	if not _expect(page.profile_list.get_root().get_child_count() == 0, "交易篩選顯示了行情設定"): return
	page._new_profile()
	if not _expect(page.purpose.selected == 1 and page.fields.id.editable, "交易篩選下新增設定未預填交易用途"): return
	page.provider_type.select(2)
	page.provider_type.item_selected.emit(2)
	page.fields.id.text = "ui-custom"
	page.fields.provider.text = "fixture_broker"
	page.fields.provider.text_changed.emit("fixture_broker")
	if not _expect(page.extras.visible and page.certificate.visible, "自訂供應商欄位未顯示"): return
	page._extra_field(false, "environment", "sandbox")
	page._extra_field(true, "api_key", "ui-fixture-sensitive")
	page.save_button.pressed.emit()
	if not _expect(page.status.text == "已儲存 API 設定。" and page.extra_rows.size() == 2, "自訂參數儲存或重讀失敗"): return
	if not _expect(page.extra_rows[1].value.secret and page.extra_rows[1].value.text.is_empty() and page.extra_rows[1].value.placeholder_text == "******", "自訂敏感值遮罩錯誤"): return
	page.extra_rows[1].visibility.toggle.pressed.emit()
	if not _expect(page.extra_rows[1].visibility.display.text == "ui-fixture-sensitive", "無法顯示已儲存的自訂敏感值"): return
	page.edit_button.pressed.emit()
	if not _expect(page.extra_rows[1].visibility.display.text.is_empty(), "進入編輯時未重新隱藏敏感值"): return
	var trade: TreeItem = page.profile_list.get_root().get_first_child()
	if not _expect(trade != null and trade.get_text(0) == "ui-custom" and trade.get_text(2) == "交易" and trade.get_next() == null, "儲存後交易設定未出現在列表"): return
	page.filter.select(0)
	page.filter.item_selected.emit(0)
	var quote: TreeItem = page.profile_list.get_root().get_first_child()
	page.fields.host.text = "unsaved"
	page.profile_list.set_block_signals(true)
	quote.select(0)
	page.profile_list.set_block_signals(false)
	page.profile_list.item_selected.emit()
	await process_frame
	if not _expect(page.discard_dialog.visible and page.fields.host.text == "unsaved" and page.selected_profile_id == "ui-custom", "切換設定時未保護未儲存草稿"): return
	page.discard_dialog.hide()
	page.discard_dialog.canceled.emit()
	quote = page.profile_list.get_root().get_first_child()
	page.profile_list.set_block_signals(true)
	quote.select(0)
	page.profile_list.set_block_signals(false)
	page.profile_list.item_selected.emit()
	await process_frame
	page.discard_dialog.hide()
	page.discard_dialog.confirmed.emit()
	if not _expect(page.selected_profile_id == "gateway-test" and page.fields.port.text == "5433" and not page.fields.id.editable, "選取清單項目未載入可編輯的行情設定"): return
	page.open_tradecom()
	if not _expect(page.fields.provider.text == "kgi_tradecom" and page.purpose.selected == 1 and page.fields.broker_id.get_parent().visible, "TradeCom 設定入口未載入交易欄位"): return
	if not _expect(page.secrets.trade_account.field.secret and page.secrets.sub_account.field.secret and not page.secrets.token.row.visible, "TradeCom 敏感欄位或表單配置錯誤"): return
	page.fields.id.text = "tradecom-test"
	page.fields.host.text = "localhost"
	page.fields.port.text = "1234"
	page.fields.broker_id.text = "0012"
	page.secrets.account.field.text = "fixture-user"
	page.secrets.password.field.text = "fixture-password"
	page.secrets.trade_account.field.text = "0001234"
	page.secrets.sub_account.field.text = "0005678"
	page.save_button.pressed.emit()
	if not _expect(page.selected_profile_id == "tradecom-test" and page.fields.broker_id.text == "0012" and page.secrets.trade_account.field.text.is_empty() and page.secrets.sub_account.field.placeholder_text == "******", "TradeCom 設定儲存或敏感欄位遮罩失敗"): return
	page.secrets.trade_account.toggle.pressed.emit()
	if not _expect(page.secrets.trade_account.display.text == "0001234", "無法顯示完整交易帳號"): return
	studio.open_page(studio.CODE)
	if not _expect(page.secrets.trade_account.display.text.is_empty(), "離開頁面後未清除顯示中的帳號"): return
	if not _expect(studio.workbench.visible and not page.is_visible_in_tree(), "返回工作台失敗"): return
	print("API_PROFILES_UI_OK")
	quit(0)

func _test_live_chart(studio: Control) -> void:
	studio.navigation.current_tab=studio.LIVE_CHART
	var view: Control=studio.live_chart
	view.broker_profile="fixture"
	if not _expect(view.graph.get_parent().get_parent() is VBoxContainer,"移除側欄後 K 線應直接使用全寬版面"): return
	view._switch_context()
	await _wait_until(func(): return view.source_codes.has("TAIFEX:TX:202612"))
	if not _expect(view.source_codes.has("EMPTY") and view.product_names.get("TAIFEX:TX:202612")=="臺股期貨","未列出歷史商品主檔"): return
	var product_index: int=view.source_codes.find("TAIFEX:TX:202612")
	view.market.select(product_index)
	view.market.item_selected.emit(product_index)
	await _wait_until(func(): return view.historical_view and view.graph.limit==2)
	if not _expect(view.graph.limit==2 and view.graph.prices[8]==19902 and view.title.text.contains("1 分 K"),"沒有即時行情時未依時間顯示歷史分 K"): return
	view.unit.select(3)
	view.unit.item_selected.emit(3)
	view.period.value=5
	await _wait_until(func(): return view.graph.limit==2 and view.title.text.contains("5 分 K"))
	if not _expect(view.graph.times[0].begins_with("2026-11-19T08:40:00") and view.graph.times[1].begins_with("2026-11-19T08:50:00"),"歷史 5 分 K 未依收棒時間彙整"): return
	view.unit.select(1)
	view.unit.item_selected.emit(1)
	await _wait_until(func(): return view.status.text.contains("尚無 Tick 行情"))
	if not _expect(view.graph.limit==0 and view.status.text.contains("尚無 Tick 行情"),"所選週期無資料時應清楚顯示"): return
	var daily_index: int=view.source_codes.find("ZZ_DAILY")
	view.unit.select(4)
	view.unit.item_selected.emit(4)
	view.period.value=2
	view.market.select(daily_index)
	view.market.item_selected.emit(daily_index)
	await _wait_until(func(): return view.historical_view and view.graph.limit==1)
	if not _expect(view.graph.limit==1 and view.graph.times[0]=="2026-11-20"
		and view.graph.prices==PackedFloat64Array([511,525,501,520,17]) and view.title.text.contains("2 日 K"),"未從歷史日 K 彙整所選日週期"): return
	view.unit.select(0)
	view.unit.item_selected.emit(0)
	view.market.select(product_index)
	view.market.item_selected.emit(product_index)
	await _wait_until(func(): return view.historical_view and view.graph.limit==2)
	print("HISTORY_CHART_OK")
	await _wait_until(func(): return view.graph.limit==1 and view.graph.pending.has(0))
	if not _expect(view.graph.limit==1 and view.graph.prices[3]==20000 and view.graph.fills.is_empty(),"初始即時 K 棒錯誤"): return
	print("LIVE_CHART_INITIAL")
	await _wait_until(func(): return view.graph.limit==1 and view.graph.prices[3]==20003 and view.graph.fills.has(0))
	if not _expect(view.graph.limit==1 and view.graph.fills.get(0,[]).size()==1 and view.graph.pending.has(0)
		and view.graph.prices[4]==3 and view.quote.tooltip_text.contains("買進"),"新 Tick 或成交標記未更新，或重複畫出成交"): return
	print("LIVE_CHART_UPDATED")
	await _wait_until(func(): return view.graph.limit==2 and view.graph.pending.has(1))
	if not _expect(view.graph.limit==2 and not view.graph.pending.has(0) and view.graph.fills.get(0,[]).size()==1
		and not view.graph.fills.has(1),"跨分鐘未正確收棒或成交標記移位"): return
	view.graph.bar_selected.emit(0)
	if not _expect(view.graph.selected==0,"無法暫時查看先前 K 棒"): return
	print("LIVE_CHART_INSPECTED")
	await _wait_until(func(): return view.graph.limit==2 and view.graph.prices[8]==20006)
	if not _expect(view.graph.limit==2 and view.graph.prices[8]==20006 and view.graph.selected==1,
		"查看先前 K 棒後沒有隨新行情回到最新棒"): return
	view.unit.select(3)
	view.unit.item_selected.emit(3)
	await _wait_until(func(): return view.graph.limit==1 and view.title.text.contains("5 分 K"))
	if not _expect(view.graph.limit==1 and view.graph.prices==PackedFloat64Array([20000,20006,20000,20006,8])
		and view.graph.pending.has(0) and view.graph.fills.get(0,[]).size()==1,"即時 5 分 K 的 OHLCV、未收棒或成交標記有誤"): return
	print("LIVE_CHART_FIVE_MINUTE")
	await _wait_until(func(): return view.graph.limit==2 and view.graph.prices[8]==20007)
	if not _expect(view.graph.limit==2 and not view.graph.pending.has(0)
		and view.graph.pending.has(1) and view.graph.fills.get(0,[]).size()==1,"即時 5 分 K 未隨新行情收棒並產生下一根"): return
	view.unit.select(2)
	view.unit.item_selected.emit(2)
	view.period.value=5
	await _wait_until(func(): return view.graph.limit==5 and view.title.text.contains("5 秒 K"))
	if not _expect(view.graph.limit==5 and view.graph.times[1].ends_with("08:45:25")
		and view.graph.pending.has(4) and view.graph.fills.get(1,[]).size()==1,"即時 5 秒 K 未依 Tick 產生或成交標記有誤"): return
	view.unit.select(1)
	view.unit.item_selected.emit(1)
	await _wait_until(func(): return view.graph.limit==5 and view.graph.tick_axis)
	if not _expect(view.graph.limit==5 and view.graph.pending.is_empty() and view.graph.times[1].ends_with(".123000000")
		and view.graph.fills.get(1,[]).size()==1,"Tick 模式未保留逐筆時間或成交標記"): return
	view.strategy_panel.zone="Asia/Taipei"
	view.strategy_panel.fills=[{"datetime":"2026-11-20T08:45:20.123","is_entry":true,"side":1,"quantity":1,"price":20003}]
	view.strategy_panel.report_loaded=true
	view.strategy_panel.overlay_changed.emit()
	if not _expect(not view.historical_view and view.graph.fills.get(1,[]).size()==2 and view.graph.fills[1][1].is_entry,
		"回測進出場應與券商成交同時顯示，並保留即時行情"): return
	view.strategy_panel.report_loaded=false
	view.strategy_panel.overlay_changed.emit()
	if not _expect(view.graph.fills.get(1,[]).size()==1,"移除回測標記後應保留券商成交"): return
	view.unit.select(0)
	view.unit.item_selected.emit(0)
	view.start_field.text="2026/11/19"
	view.end_field.text="2026/11/19"
	view._apply_dates()
	await _wait_until(func(): return view.historical_view and view.graph.limit==2)
	if not _expect(view.historical_view and view.graph.full_range and view.graph.first==0 and view.graph.limit==2 and view.graph.times[1].begins_with("2026-11-19") and view.status.text.contains("2026/11/19 ～ 2026/11/19"),
		"指定同日起訖應讀取當天歷史，排除下一日的即時資料"): return
	view.start_field.text="2026/11/20"
	view._apply_dates()
	if not _expect(view.range_error.visible and view.start_date=="2026-11-19" and view.graph.limit==2,"起迄顛倒時應保留上次圖表"): return
	view.start_field.text="2026/02/29"
	view._apply_dates()
	if not _expect(view.range_error.visible and view.start_date=="2026-11-19","不應接受無效曆日"): return
	view.start_field.text="2026/11/18"
	view.end_field.text="2026/11/18"
	view._apply_dates()
	await _wait_until(func(): return view.status.text.contains("尚無") and view.status.text.contains("2026/11/18"))
	if not _expect(view.graph.limit==0 and view.status.text.contains("尚無"),"日期沒有行情時應顯示空狀態"): return
	view._clear_dates()
	await _wait_until(func(): return not view.historical_view and view.graph.limit==3)
	if not _expect(view.graph.limit==3 and view.start_date.is_empty() and view.end_date.is_empty() and not view.range_error.visible,
		"清除日期應恢復最新行情"): return
	if not _expect(not view.get_property_list().any(func(property): return property.name=="source"),
		"即時圖表不應再有行情來源選擇器"): return
	print("LIVE_CHART_OK")
	quit(0)

func _expect(condition: bool, message: String) -> bool:
	if not condition:
		push_error(message)
		quit(1)
	return condition

func _wait_until(predicate: Callable, timeout_ms := 60_000) -> void:
	var deadline := Time.get_ticks_msec()+timeout_ms
	while not predicate.call() and Time.get_ticks_msec()<deadline: await process_frame
