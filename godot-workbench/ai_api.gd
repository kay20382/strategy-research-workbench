extends RefCounted

const NAMES := {"openai":"OpenAI","anthropic":"Claude","gemini":"Gemini","grok":"Grok","mixroute":"MixRoute"}
const ENV_KEYS := {
	"openai":"OPENAI_API_KEY","anthropic":"ANTHROPIC_API_KEY","gemini":"GEMINI_API_KEY",
	"grok":"XAI_API_KEY","mixroute":"MIXROUTE_API_KEY"
}
const MODELS := {
	"openai":["gpt-5-mini","gpt-5"],
	"anthropic":["claude-sonnet-5-5","claude-haiku-5-5","claude-opus-5-5"],
	"gemini":["gemini-3.8-flash","gemini-3.6-flash","gemini-3.5-flash-lite"],
	"grok":["grok-4.7","grok-4.6","grok-4.5","grok-4.3"],
	"mixroute":["gemini-3.6-flash","claude-sonnet-5-5","grok-4.3","gpt-5.4","gpt-6.1-sol","gpt-6.2-sol"]
}

static func available_models(provider: String, configured: Array) -> Array[String]:
	var models: Array[String]=[]
	for value in configured+MODELS[provider]:
		var name: String=str(value).strip_edges()
		if not name.is_empty() and name not in models:
			models.append(name)
	return models

static func headers(provider: String, key: String) -> PackedStringArray:
	var output := PackedStringArray(["Content-Type: application/json"])
	match provider:
		"openai", "grok", "mixroute": output.append("Authorization: Bearer "+key)
		"anthropic": output.append_array(["Authorization: Bearer "+key,"anthropic-version: 2026-10-21"])
		"gemini": output.append("x-goog-api-key: "+key)
	return output

static func endpoint(provider: String, model: String) -> String:
	match provider:
		"openai": return "https://api.openai.com/v1/responses"
		"anthropic": return "https://api.anthropic.com/v1/messages"
		"gemini": return "https://generativelanguage.googleapis.com/v1beta/models/%s:generateContent" % model.trim_prefix("models/").uri_encode()
		"grok": return "https://api.x.ai/v1/chat/completions"
		"mixroute": return "https://api.mixroute.ai/v1/chat/completions"
	return ""

static func payload_for(provider: String, payload: Dictionary) -> Dictionary:
	match provider:
		"grok", "mixroute":
			var format: Dictionary=payload.text.format
			return {"model":payload.model,"max_tokens":payload.max_output_tokens,
				"messages":[{"role":"system","content":payload.instructions},{"role":"user","content":payload.input}],
				"response_format":{"type":"json_schema","json_schema":{
					"name":format.name,"schema":format.schema,"strict":true}}}
		"anthropic":
			return {"model":payload.model,"max_tokens":payload.max_output_tokens,
				"system":payload.instructions,"messages":[{"role":"user","content":payload.input}],
				"output_config":{"format":{"type":"json_schema","schema":payload.text.format.schema}}}
		"gemini":
			return {"systemInstruction":{"parts":[{"text":payload.instructions}]},
				"contents":[{"role":"user","parts":[{"text":payload.input}]}],
				"generationConfig":{"maxOutputTokens":payload.max_output_tokens,
					"responseFormat":{"text":{"mimeType":"APPLICATION_JSON","schema":payload.text.format.schema}}}}
	return payload

static func read_json(provider: String, body: PackedByteArray) -> Variant:
	var response: Variant=JSON.parse_string(body.get_string_from_utf8())
	if not response is Dictionary:
		return null
	var output := ""
	match provider:
		"openai":
			if response.get("status")!="completed":
				return null
			for item in response.get("output",[]):
				if item.get("type")=="message":
					for part in item.get("content",[]):
						if part.get("type")=="output_text":
							output+=str(part.get("text",""))
		"anthropic":
			if response.get("stop_reason")!="end_turn":
				return null
			for part in response.get("content",[]):
				if part.get("type")=="text":
					output+=str(part.get("text",""))
		"gemini":
			var options: Array=response.get("candidates",[])
			if options.is_empty() or options[0].get("finishReason")!="STOP":
				return null
			for part in options[0].get("content",{}).get("parts",[]):
				if not part.get("thought",false):
					output+=str(part.get("text",""))
		"grok", "mixroute":
			var choices: Array=response.get("choices",[])
			if choices.is_empty() or choices[0].get("finish_reason")!="stop":
				return null
			output=str(choices[0].get("message",{}).get("content",""))
	return JSON.parse_string(output) if not output.is_empty() else null
