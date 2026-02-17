#!/usr/bin/env bash
set -euo pipefail

# Real upstream capability matrix:
# - upstream type: chat_completion, responses
# - downstream: /v1/chat/completions, /v1/responses, /v1/messages
# - checks: non-stream, stream text, reasoning, tool call, stream tool call,
#           parallel tool call, tool-result roundtrip,
#           structured output (chat/responses), structured stream (chat/responses)

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT_DIR"

UPSTREAM_BASE_URL=${UPSTREAM_BASE_URL:-http://127.0.0.1:4141/v1}
MODEL=${MODEL:-gpt-5-mini}
UPSTREAM_KEY=${UPSTREAM_KEY:-any_key}
PORT_CHAT=${PORT_CHAT:-18081}
PORT_RESP=${PORT_RESP:-18082}
REQ_TIMEOUT=${REQ_TIMEOUT:-20}

AUTH_HEADER=""
CT_HEADER="Content-Type: application/json"

have() { command -v "$1" >/dev/null 2>&1; }
require() {
  if ! have "$1"; then
    echo "missing command: $1" >&2
    exit 1
  fi
}

require curl
require jq
require grep

contains() {
  local pattern="$1"
  local data="$2"
  grep -Eq "$pattern" <<<"$data"
}

post_json() {
  local url="$1"
  local body="$2"
  local out
  out=$(curl -sS --max-time "${REQ_TIMEOUT}" "$url" -H "$AUTH_HEADER" -H "$CT_HEADER" -d "$body" 2>/dev/null || true)
  if [[ -z "$out" ]]; then
    echo '{"error":{"code":"curl_failed"}}'
  else
    echo "$out"
  fi
}

sample_stream_post() {
  local url="$1"
  local body="$2"
  # Capture an early sample only; stream may be cut by head.
  (curl -sN --max-time "${REQ_TIMEOUT}" "$url" -H "$AUTH_HEADER" -H "$CT_HEADER" -d "$body" | head -n 280) 2>/dev/null || true
}

start_monoize_for_upstream() {
  local upstream_type="$1"
  local port="$2"
  local tmpd
  tmpd=$(mktemp -d)

  MONOIZE_LISTEN="127.0.0.1:${port}" \
    MONOIZE_DATABASE_DSN="sqlite://${tmpd}/monoize.db" \
    ./target/debug/monoize >"/tmp/monoize-matrix-${upstream_type}.log" 2>&1 &
  local pid=$!

  for _ in $(seq 1 80); do
    if curl -sS --max-time 2 "http://127.0.0.1:${port}/metrics" >/dev/null 2>&1; then
      break
    fi
    sleep 0.2
  done

  local admin_user="matrix_admin_${port}"
  local admin_pass="matrix_pass_123"
  local register_resp
  register_resp=$(curl -sS --max-time "${REQ_TIMEOUT}" \
    "http://127.0.0.1:${port}/api/dashboard/auth/register" \
    -H "$CT_HEADER" \
    -d "{\"username\":\"${admin_user}\",\"password\":\"${admin_pass}\"}")
  local session_token
  session_token=$(printf '%s' "$register_resp" | jq -r '.token // empty')
  local user_id
  user_id=$(printf '%s' "$register_resp" | jq -r '.user.id // empty')
  if [[ -z "$session_token" || -z "$user_id" ]]; then
    echo "failed to bootstrap admin user" >&2
    stop_monoize "$pid" "$tmpd"
    exit 1
  fi

  curl -sS --max-time "${REQ_TIMEOUT}" \
    "http://127.0.0.1:${port}/api/dashboard/users/${user_id}" \
    -H "Authorization: Bearer ${session_token}" \
    -H "$CT_HEADER" \
    -X PUT \
    -d '{"balance_unlimited":true}' >/dev/null

  local key_resp
  key_resp=$(curl -sS --max-time "${REQ_TIMEOUT}" \
    "http://127.0.0.1:${port}/api/dashboard/tokens" \
    -H "Authorization: Bearer ${session_token}" \
    -H "$CT_HEADER" \
    -d '{"name":"matrix-key"}')
  local forward_token
  forward_token=$(printf '%s' "$key_resp" | jq -r '.key // empty')
  if [[ -z "$forward_token" ]]; then
    echo "failed to create forwarding api key" >&2
    stop_monoize "$pid" "$tmpd"
    exit 1
  fi

  local provider_body
  provider_body=$(jq -nc \
    --arg model "$MODEL" \
    --arg upstream_type "$upstream_type" \
    --arg base_url "$UPSTREAM_BASE_URL" \
    --arg upstream_key "$UPSTREAM_KEY" \
    '{
      name:"up",
      provider_type:$upstream_type,
      models:{($model):{multiplier:1}},
      channels:[{name:"up-default", base_url:$base_url, api_key:$upstream_key}]
    }')
  curl -sS --max-time "${REQ_TIMEOUT}" \
    "http://127.0.0.1:${port}/api/dashboard/providers" \
    -H "Authorization: Bearer ${session_token}" \
    -H "$CT_HEADER" \
    -d "$provider_body" >/dev/null

  echo "$pid|$tmpd|$forward_token"
}

stop_monoize() {
  local pid="$1"
  local tmpd="$2"
  kill "$pid" >/dev/null 2>&1 || true
  wait "$pid" >/dev/null 2>&1 || true
  rm -rf "$tmpd"
}

run_case() {
  local upstream_type="$1"
  local port="$2"

  local info pid tmpd token
  info=$(start_monoize_for_upstream "$upstream_type" "$port")
  pid=${info%%|*}
  tmpd=${info#*|}
  token=${tmpd#*|}
  tmpd=${tmpd%%|*}
  AUTH_HEADER="Authorization: Bearer ${token}"

  local base="http://127.0.0.1:${port}/v1"

  # Chat downstream
  local chat_non_req chat_non chat_stream_req chat_stream
  local chat_tool_req chat_tool1 chat_tool_calls chat_tool_count chat_tool2_req chat_tool2 chat_tool_stream
  local chat_struct_req chat_struct chat_struct_stream

  chat_non_req=$(jq -nc --arg model "$MODEL" '{model:$model, reasoning_effort:"high", messages:[{role:"user", content:"Answer briefly: what is 12*13?"}]}')
  chat_non=$(post_json "$base/chat/completions" "$chat_non_req")

  chat_stream_req=$(jq -nc --arg model "$MODEL" '{model:$model, stream:true, reasoning_effort:"high", messages:[{role:"user", content:"Think and answer 17*19."}]}')
  chat_stream=$(sample_stream_post "$base/chat/completions" "$chat_stream_req")

  chat_tool_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    parallel_tool_calls:true,
    tool_choice:"required",
    tools:[
      {type:"function", function:{name:"websearch", parameters:{type:"object", properties:{query:{type:"string"}}, required:["query"]}}},
      {type:"function", function:{name:"weather", parameters:{type:"object", properties:{city:{type:"string"}}, required:["city"]}}}
    ],
    messages:[{role:"user", content:"Call both websearch and weather in parallel first, then wait for tool results."}]
  }')
  chat_tool1=$(post_json "$base/chat/completions" "$chat_tool_req")
  chat_tool_calls=$(printf '%s' "$chat_tool1" | jq -c '.choices[0].message.tool_calls // []' 2>/dev/null || echo '[]')
  chat_tool_count=$(printf '%s' "$chat_tool_calls" | jq 'length' 2>/dev/null || echo 0)
  chat_tool2='{}'
  if [[ "$chat_tool_count" -gt 0 ]]; then
    chat_tool2_req=$(jq -nc --arg model "$MODEL" --argjson calls "$chat_tool_calls" '{
      model:$model,
      messages: ([{role:"assistant", content:"", tool_calls:$calls}] + ($calls | map({
        role:"tool",
        tool_call_id:.id,
        content:(if .function.name=="weather" then "STUB WEATHER RESULT: sunny 25C" else "STUB WEB RESULT: latest Anthropic Opus is Opus 4.1" end)
      })))
    }')
    chat_tool2=$(post_json "$base/chat/completions" "$chat_tool2_req")
  fi
  chat_tool_stream=$(sample_stream_post "$base/chat/completions" "$(jq -nc --arg model "$MODEL" --argjson t "$(printf '%s' "$chat_tool_req")" '$t + {stream:true}')")

  chat_struct_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    response_format:{
      type:"json_schema",
      json_schema:{name:"x", strict:true, schema:{type:"object", properties:{model:{type:"string"}}, required:["model"], additionalProperties:false}}
    },
    messages:[{role:"user", content:"Return JSON only."}]
  }')
  chat_struct=$(post_json "$base/chat/completions" "$chat_struct_req")
  chat_struct_stream=$(sample_stream_post "$base/chat/completions" "$(jq -nc --argjson t "$(printf '%s' "$chat_struct_req")" '$t + {stream:true}')")

  # Responses downstream
  local resp_non_req resp_non resp_stream_req resp_stream
  local resp_tool_req resp_tool1 resp_tool_calls resp_tool_count resp_tool2_req resp_tool2 resp_tool_stream
  local resp_struct_req resp_struct resp_struct_stream

  resp_non_req=$(jq -nc --arg model "$MODEL" '{model:$model, reasoning:{effort:"high"}, input:"Answer briefly: what is 21*22?"}')
  resp_non=$(post_json "$base/responses" "$resp_non_req")

  resp_stream_req=$(jq -nc --arg model "$MODEL" '{model:$model, stream:true, reasoning:{effort:"high"}, input:"Think and answer 23*24."}')
  resp_stream=$(sample_stream_post "$base/responses" "$resp_stream_req")

  resp_tool_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    parallel_tool_calls:true,
    tool_choice:"required",
    tools:[
      {type:"function", name:"websearch", parameters:{type:"object", properties:{query:{type:"string"}}, required:["query"]}},
      {type:"function", name:"weather", parameters:{type:"object", properties:{city:{type:"string"}}, required:["city"]}}
    ],
    input:"Call both websearch and weather in parallel first, then wait for tool results."
  }')
  resp_tool1=$(post_json "$base/responses" "$resp_tool_req")
  resp_tool_calls=$(printf '%s' "$resp_tool1" | jq -c '[.output[]? | select(.type=="function_call")]' 2>/dev/null || echo '[]')
  resp_tool_count=$(printf '%s' "$resp_tool_calls" | jq 'length' 2>/dev/null || echo 0)
  resp_tool2='{}'
  if [[ "$resp_tool_count" -gt 0 ]]; then
    resp_tool2_req=$(jq -nc --arg model "$MODEL" --argjson calls "$resp_tool_calls" '{
      model:$model,
      input:(
        ($calls | map({type:"function_call", call_id:.call_id, name:.name, arguments:.arguments})) +
        ($calls | map({type:"function_call_output", call_id:.call_id, output:(if .name=="weather" then "STUB WEATHER RESULT: sunny 25C" else "STUB WEB RESULT: latest Anthropic Opus is Opus 4.1" end)}))
      )
    }')
    resp_tool2=$(post_json "$base/responses" "$resp_tool2_req")
  fi
  resp_tool_stream=$(sample_stream_post "$base/responses" "$(jq -nc --argjson t "$(printf '%s' "$resp_tool_req")" '$t + {stream:true}')")

  resp_struct_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    response_format:{
      type:"json_schema",
      json_schema:{name:"x", strict:true, schema:{type:"object", properties:{model:{type:"string"}}, required:["model"], additionalProperties:false}}
    },
    input:"Return JSON only."
  }')
  resp_struct=$(post_json "$base/responses" "$resp_struct_req")
  resp_struct_stream=$(sample_stream_post "$base/responses" "$(jq -nc --argjson t "$(printf '%s' "$resp_struct_req")" '$t + {stream:true}')")

  # Messages downstream
  local msg_non_req msg_non msg_stream_req msg_stream
  local msg_tool_req msg_tool1 msg_tool_uses msg_tool_count msg_tool2_req msg_tool2 msg_tool_stream

  msg_non_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    max_tokens:512,
    thinking:{type:"enabled", budget_tokens:1024},
    messages:[{role:"user", content:"Answer briefly: what is 25*26?"}]
  }')
  msg_non=$(post_json "$base/messages" "$msg_non_req")

  msg_stream_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    max_tokens:512,
    stream:true,
    thinking:{type:"enabled", budget_tokens:1024},
    messages:[{role:"user", content:"Think and answer 27*28."}]
  }')
  msg_stream=$(sample_stream_post "$base/messages" "$msg_stream_req")

  msg_tool_req=$(jq -nc --arg model "$MODEL" '{
    model:$model,
    max_tokens:512,
    parallel_tool_calls:true,
    tool_choice:{type:"any"},
    tools:[
      {name:"websearch", input_schema:{type:"object", properties:{query:{type:"string"}}, required:["query"]}},
      {name:"weather", input_schema:{type:"object", properties:{city:{type:"string"}}, required:["city"]}}
    ],
    messages:[{role:"user", content:"Call both websearch and weather in parallel first, then wait for tool results."}]
  }')
  msg_tool1=$(post_json "$base/messages" "$msg_tool_req")
  msg_tool_uses=$(printf '%s' "$msg_tool1" | jq -c '[.content[]? | select(.type=="tool_use")]' 2>/dev/null || echo '[]')
  msg_tool_count=$(printf '%s' "$msg_tool_uses" | jq 'length' 2>/dev/null || echo 0)
  msg_tool2='{}'
  if [[ "$msg_tool_count" -gt 0 ]]; then
    msg_tool2_req=$(jq -nc --arg model "$MODEL" --argjson uses "$msg_tool_uses" '{
      model:$model,
      max_tokens:512,
      messages:[
        {role:"assistant", content:$uses},
        {role:"user", content:($uses | map({type:"tool_result", tool_use_id:.id, content:(if .name=="weather" then "STUB WEATHER RESULT: sunny 25C" else "STUB WEB RESULT: latest Anthropic Opus is Opus 4.1" end)}))}
      ]
    }')
    msg_tool2=$(post_json "$base/messages" "$msg_tool2_req")
  fi
  msg_tool_stream=$(sample_stream_post "$base/messages" "$(jq -nc --argjson t "$(printf '%s' "$msg_tool_req")" '$t + {stream:true}')")

  # Build capability object
  jq -n \
    --arg upstream "$upstream_type" \
    --arg chat_non_err "$(printf '%s' "$chat_non" | jq -r '.error.code // ""' 2>/dev/null || echo 'invalid_json')" \
    --arg chat_stream_ok "$(contains '\\[DONE\\]|"finish_reason"' "$chat_stream" && echo true || echo false)" \
    --arg chat_reasoning_non "$(printf '%s' "$chat_non" | jq -r '((.choices[0].message.reasoning // "")|length>0) or ([.choices[0].message.reasoning_details[]?]|length>0)' 2>/dev/null || echo false)" \
    --arg chat_reasoning_stream "$(contains 'reasoning_details|\"reasoning\"' "$chat_stream" && echo true || echo false)" \
    --arg chat_tool_ok "$(printf '%s' "$chat_tool1" | jq -r '[.choices[0].message.tool_calls[]?]|length>0' 2>/dev/null || echo false)" \
    --arg chat_tool_parallel "$(printf '%s' "$chat_tool1" | jq -r '[.choices[0].message.tool_calls[]?]|length>1' 2>/dev/null || echo false)" \
    --arg chat_tool_return_ok "$(printf '%s' "$chat_tool2" | jq -r '((.choices[0].message.content // "")|tostring|length)>0' 2>/dev/null || echo false)" \
    --arg chat_tool_stream_ok "$(contains '"tool_calls"' "$chat_tool_stream" && echo true || echo false)" \
    --arg chat_struct_ok "$(printf '%s' "$chat_struct" | jq -r '.error.code // "" | length == 0' 2>/dev/null || echo false)" \
    --arg chat_struct_stream_ok "$(contains '\\[DONE\\]|"finish_reason"' "$chat_struct_stream" && echo true || echo false)" \
    --arg resp_non_err "$(printf '%s' "$resp_non" | jq -r '.error.code // ""' 2>/dev/null || echo 'invalid_json')" \
    --arg resp_stream_ok "$(contains 'event: response.output_text.delta|event: response.completed' "$resp_stream" && echo true || echo false)" \
    --arg resp_reasoning_non "$(printf '%s' "$resp_non" | jq -r '[.output[]? | select(.type=="reasoning")]|length>0' 2>/dev/null || echo false)" \
    --arg resp_reasoning_stream "$(contains 'event: response.reasoning_text.delta|event: response.reasoning_signature.delta|"type":"reasoning"' "$resp_stream" && echo true || echo false)" \
    --arg resp_tool_ok "$(printf '%s' "$resp_tool1" | jq -r '[.output[]? | select(.type=="function_call")]|length>0' 2>/dev/null || echo false)" \
    --arg resp_tool_parallel "$(printf '%s' "$resp_tool1" | jq -r '[.output[]? | select(.type=="function_call")]|length>1' 2>/dev/null || echo false)" \
    --arg resp_tool_return_ok "$(printf '%s' "$resp_tool2" | jq -r '[.output[]? | select(.type=="message")]|length>0' 2>/dev/null || echo false)" \
    --arg resp_tool_stream_ok "$(contains 'event: response.function_call_arguments.delta|event: response.output_item.added' "$resp_tool_stream" && echo true || echo false)" \
    --arg resp_struct_ok "$(printf '%s' "$resp_struct" | jq -r '.error.code // "" | length == 0' 2>/dev/null || echo false)" \
    --arg resp_struct_stream_ok "$(contains 'event: response.output_text.delta|event: response.completed' "$resp_struct_stream" && echo true || echo false)" \
    --arg msg_non_err "$(printf '%s' "$msg_non" | jq -r '.error.code // ""' 2>/dev/null || echo 'invalid_json')" \
    --arg msg_stream_ok "$(contains '"type":"message_stop"|"type": "message_stop"' "$msg_stream" && echo true || echo false)" \
    --arg msg_reasoning_non "$(printf '%s' "$msg_non" | jq -r '[.content[]? | select(.type=="thinking")]|length>0' 2>/dev/null || echo false)" \
    --arg msg_reasoning_stream "$(contains '"thinking_delta"|"signature_delta"' "$msg_stream" && echo true || echo false)" \
    --arg msg_tool_ok "$(printf '%s' "$msg_tool1" | jq -r '[.content[]? | select(.type=="tool_use")]|length>0' 2>/dev/null || echo false)" \
    --arg msg_tool_parallel "$(printf '%s' "$msg_tool1" | jq -r '[.content[]? | select(.type=="tool_use")]|length>1' 2>/dev/null || echo false)" \
    --arg msg_tool_return_ok "$(printf '%s' "$msg_tool2" | jq -r '[.content[]? | select(.type=="text")]|length>0' 2>/dev/null || echo false)" \
    --arg msg_tool_stream_ok "$(contains '"input_json_delta"|"tool_use"' "$msg_tool_stream" && echo true || echo false)" \
    '{
      upstream:$upstream,
      chat:{
        nonstream_ok:($chat_non_err==""),
        stream_output_ok:($chat_stream_ok=="true"),
        reasoning_nonstream_ok:($chat_reasoning_non=="true"),
        reasoning_stream_ok:($chat_reasoning_stream=="true"),
        tool_call_ok:($chat_tool_ok=="true"),
        parallel_tool_call_ok:($chat_tool_parallel=="true"),
        tool_result_roundtrip_ok:($chat_tool_return_ok=="true"),
        stream_tool_call_ok:($chat_tool_stream_ok=="true"),
        structured_ok:($chat_struct_ok=="true"),
        structured_stream_ok:($chat_struct_stream_ok=="true")
      },
      responses:{
        nonstream_ok:($resp_non_err==""),
        stream_output_ok:($resp_stream_ok=="true"),
        reasoning_nonstream_ok:($resp_reasoning_non=="true"),
        reasoning_stream_ok:($resp_reasoning_stream=="true"),
        tool_call_ok:($resp_tool_ok=="true"),
        parallel_tool_call_ok:($resp_tool_parallel=="true"),
        tool_result_roundtrip_ok:($resp_tool_return_ok=="true"),
        stream_tool_call_ok:($resp_tool_stream_ok=="true"),
        structured_ok:($resp_struct_ok=="true"),
        structured_stream_ok:($resp_struct_stream_ok=="true")
      },
      messages:{
        nonstream_ok:($msg_non_err==""),
        stream_output_ok:($msg_stream_ok=="true"),
        reasoning_nonstream_ok:($msg_reasoning_non=="true"),
        reasoning_stream_ok:($msg_reasoning_stream=="true"),
        tool_call_ok:($msg_tool_ok=="true"),
        parallel_tool_call_ok:($msg_tool_parallel=="true"),
        tool_result_roundtrip_ok:($msg_tool_return_ok=="true"),
        stream_tool_call_ok:($msg_tool_stream_ok=="true"),
        structured_ok:null,
        structured_stream_ok:null
      }
    }'

  stop_monoize "$pid" "$tmpd"
}

chat_case=$(run_case chat_completion "$PORT_CHAT")
resp_case=$(run_case responses "$PORT_RESP")

jq -n --argjson a "$chat_case" --argjson b "$resp_case" '{matrix:[$a,$b]}'
