#!/usr/bin/env bash
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}"
HARNESS="${HARNESS:-$ROOT/target/debug/gateway-chat-harness}"
REPORT_DIR="${REPORT_DIR:-/tmp/synapseclaw-phase410-live-$(date +%s)}"
TIMEOUT_SECS="${TIMEOUT_SECS:-180}"
JOURNAL_UNIT="${JOURNAL_UNIT:-synapseclaw.service}"
RUN_ID="${RUN_ID:-$(date +%s)}"
RUN_MAIN_ROUTE="${RUN_MAIN_ROUTE:-1}"
RUN_REASONER="${RUN_REASONER:-0}"
RUN_CJK="${RUN_CJK:-1}"
RUN_MEDIA="${RUN_MEDIA:-1}"
RUN_HEAVY="${RUN_HEAVY:-0}"
RUN_DOCTOR_MODELS="${RUN_DOCTOR_MODELS:-0}"
REQUIRE_EMBEDDING_SIGNAL="${REQUIRE_EMBEDDING_SIGNAL:-0}"
STRICT_RECALL_NO_MUTATION="${STRICT_RECALL_NO_MUTATION:-0}"
STRICT_CONTEXT_BUDGET="${STRICT_CONTEXT_BUDGET:-0}"
CONTEXT_WARN_MAX_CHARS="${CONTEXT_WARN_MAX_CHARS:-7000}"
LOAD_SYSTEMD_ENV="${LOAD_SYSTEMD_ENV:-1}"
SYSTEMD_ENV_FILE="${SYSTEMD_ENV_FILE:-$HOME/.config/systemd/user/synapseclaw.env}"

mkdir -p "$REPORT_DIR"

SUMMARY="$REPORT_DIR/summary.tsv"
CONTEXT_TSV="$REPORT_DIR/provider_context.tsv"
JOURNAL_LOG="$REPORT_DIR/journal.log"
SINCE="$(date --iso-8601=seconds)"
FAILURES=0
WARNINGS=0

printf 'status\tcase\tdetail\n' > "$SUMMARY"

ensure_harness() {
  if [[ -x "$HARNESS" ]]; then
    return
  fi
  cargo build --manifest-path "$ROOT/dev/gateway-chat-harness/Cargo.toml"
}

slugify() {
  printf '%s' "$1" | tr -c '[:alnum:]_.-' '_' | sed 's/_\+/_/g; s/^_//; s/_$//'
}

record_pass() {
  printf 'PASS\t%s\t%s\n' "$1" "$2" | tee -a "$SUMMARY"
}

record_warn() {
  WARNINGS=$((WARNINGS + 1))
  printf 'WARN\t%s\t%s\n' "$1" "$2" | tee -a "$SUMMARY"
}

record_fail() {
  FAILURES=$((FAILURES + 1))
  printf 'FAIL\t%s\t%s\n' "$1" "$2" | tee -a "$SUMMARY"
}

run_case() {
  local case_name="$1"
  local route="$2"
  local session="$3"
  shift 3

  local out="$REPORT_DIR/${case_name}.json"
  local args=(--json --route "$route" --session "$session" --timeout-secs "$TIMEOUT_SECS")
  for message in "$@"; do
    args+=(-m "$message")
  done

  if "$HARNESS" "${args[@]}" > "$out" 2> "$out.stderr"; then
    record_pass "$case_name" "harness completed route=$route session=$session out=$out"
  else
    record_fail "$case_name" "harness exited non-zero route=$route session=$session stderr=$out.stderr"
  fi
}

case_field() {
  local out="$1"
  local field="$2"
  python3 - "$out" "$field" <<'PY'
import json
import sys

path, field = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

turns = data.get("turns") or []
last = turns[-1] if turns else {}
events = last.get("events") or []

def event_text(ev):
    value = ev.get("content")
    if isinstance(value, str):
        return value
    return json.dumps(value, ensure_ascii=False)

if field == "assistant":
    print("\n".join(event_text(ev) for ev in events if ev.get("type") == "assistant"))
elif field == "rpc_error":
    errors = [turn.get("rpc_error") for turn in turns if turn.get("rpc_error")]
    print("\n".join(errors))
elif field == "last_rpc_error":
    print(last.get("rpc_error") or "")
elif field == "last_tool_calls":
    print("\n".join(event_text(ev) for ev in events if ev.get("type") == "tool_call"))
elif field == "tool_call_count":
    print(sum(1 for ev in events if ev.get("type") == "tool_call"))
elif field == "all_text":
    print(json.dumps(data, ensure_ascii=False))
else:
    raise SystemExit(f"unknown field: {field}")
PY
}

assert_contains() {
  local case_name="$1"
  local out="$2"
  local needle="$3"
  local haystack
  haystack="$(case_field "$out" all_text)"
  if [[ "$haystack" == *"$needle"* ]]; then
    record_pass "$case_name" "found expected marker: $needle"
  else
    record_fail "$case_name" "missing expected marker: $needle"
  fi
}

assert_assistant_contains() {
  local case_name="$1"
  local out="$2"
  local needle="$3"
  local assistant
  assistant="$(case_field "$out" assistant)"
  if [[ "$assistant" == *"$needle"* ]]; then
    record_pass "$case_name" "assistant contains: $needle"
  else
    record_fail "$case_name" "assistant missing: $needle"
  fi
}

check_no_recall_mutation() {
  local case_name="$1"
  local out="$2"
  local last_tool_calls
  last_tool_calls="$(case_field "$out" last_tool_calls)"
  if [[ "$last_tool_calls" == *"core_memory_update"* ]]; then
    if [[ "$STRICT_RECALL_NO_MUTATION" == "1" ]]; then
      record_fail "$case_name" "recall turn emitted core_memory_update"
    else
      record_warn "$case_name" "recall turn emitted core_memory_update"
    fi
  else
    record_pass "$case_name" "recall turn did not emit core_memory_update"
  fi
}

run_provider_smoke() {
  local route="$1"
  local slug
  slug="$(slugify "$route")"
  local session="phase410-${slug}-${RUN_ID}"
  local hello_case="${slug}_hello"
  local tool_case="${slug}_tool"
  local memory_case="${slug}_memory"
  local tool_file="/tmp/scw-phase410-${slug}-${RUN_ID}"

  rm -f "$tool_file"

  run_case "$hello_case" "$route" "$session-hello" "Reply with exactly HELLO."
  assert_assistant_contains "$hello_case" "$REPORT_DIR/${hello_case}.json" "HELLO"

  run_case "$tool_case" "$route" "$session-tool" \
    "Use the shell tool to run exactly: touch $tool_file . After the tool succeeds, reply exactly TOOL_OK."
  if [[ -e "$tool_file" ]]; then
    record_pass "$tool_case" "tool-created $tool_file"
  else
    record_fail "$tool_case" "tool file missing: $tool_file"
  fi

  local branch="release/phase410-${slug}-${RUN_ID}"
  local url="https://phase410-${slug}-${RUN_ID}.invalid"
  local risk="context-regression-${slug}-${RUN_ID}"
  run_case "$memory_case" "$route" "$session-memory" \
    "Remember for this working chain: project Phase410-${slug}, branch $branch, staging URL $url, main risk $risk." \
    "For this exact working chain, what branch, staging URL, and main risk did I ask you to remember? Answer briefly."
  assert_assistant_contains "$memory_case" "$REPORT_DIR/${memory_case}.json" "$branch"
  assert_assistant_contains "$memory_case" "$REPORT_DIR/${memory_case}.json" "$url"
  assert_assistant_contains "$memory_case" "$REPORT_DIR/${memory_case}.json" "$risk"
  check_no_recall_mutation "$memory_case" "$REPORT_DIR/${memory_case}.json"
}

run_cjk_smoke() {
  local route="$1"
  local slug
  slug="$(slugify "$route")"
  local case_name="${slug}_cjk"
  local session="phase410-cjk-${slug}-${RUN_ID}"

  run_case "$case_name" "$route" "$session" \
    "记住当前工作链：项目 青龙-${RUN_ID}，分支 feature/支付修复-${slug}，风险 登录回调循环-${RUN_ID}。" \
    "当前工作链的项目、分支和风险是什么？请简短回答。"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "青龙-${RUN_ID}"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "feature/支付修复-${slug}"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "登录回调循环-${RUN_ID}"
}

run_media_admission_smoke() {
  local route="${MEDIA_ROUTE:-cheap}"
  local slug
  slug="$(slugify "$route")"
  local markers=("IMAGE" "AUDIO" "VIDEO" "MUSIC")

  for marker in "${markers[@]}"; do
    local marker_lower
    marker_lower="$(printf '%s' "$marker" | tr '[:upper:]' '[:lower:]')"
    local case_name="media_${marker_lower}_${slug}"
    run_case "$case_name" "$route" "phase410-media-${marker_lower}-${RUN_ID}" \
      "[GENERATE:${marker}] Phase 4.10 admission test only. Do not fake a media artifact; use the configured lane or fail early."
    local rpc_error
    rpc_error="$(case_field "$REPORT_DIR/${case_name}.json" last_rpc_error)"
    if [[ "$rpc_error" == *"turn admission blocked provider call"* ]]; then
      record_pass "$case_name" "blocked before provider call on text route"
    elif [[ -n "$rpc_error" ]]; then
      record_warn "$case_name" "unexpected rpc_error: $rpc_error"
    else
      record_warn "$case_name" "not blocked; inspect admission logs for lane route"
    fi
  done

  local vision_case="media_understanding_${slug}"
  local vision_data_uri="${VISION_TEST_IMAGE_DATA_URI:-data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAFElEQVR4nGP4TyJgGNUwqmH4agAAr639H708R/EAAAAASUVORK5CYII=}"
  local vision_expected="${VISION_TEST_EXPECTED_TEXT:-White}"
  run_case "$vision_case" "$route" "phase410-vision-${RUN_ID}" \
    "[IMAGE:${vision_data_uri}] What is the dominant color of this image? Reply with exactly one word."
  local vision_error
  vision_error="$(case_field "$REPORT_DIR/${vision_case}.json" last_rpc_error)"
  if [[ "$vision_error" == *"turn admission blocked provider call"* ]]; then
    record_pass "$vision_case" "blocked before provider call on non-vision route"
  elif [[ -n "$vision_error" ]]; then
    record_warn "$vision_case" "unexpected rpc_error: $vision_error"
  else
    local vision_tool_count
    vision_tool_count="$(case_field "$REPORT_DIR/${vision_case}.json" tool_call_count)"
    if [[ "$vision_tool_count" == "0" ]]; then
      record_pass "$vision_case" "vision-capable route answered without tool archaeology"
      if [[ -n "$vision_expected" ]]; then
        assert_assistant_contains "$vision_case" "$REPORT_DIR/${vision_case}.json" "$vision_expected"
      fi
    else
      record_fail "$vision_case" "vision-capable route emitted $vision_tool_count tool call(s)"
    fi
  fi
}

run_heavy_dialogue() {
  local route="${HEAVY_ROUTE:-cheap}"
  local slug
  slug="$(slugify "$route")"
  local case_name="long_semantic_${slug}"
  local session="phase410-long-semantic-${slug}-${RUN_ID}"
  local args=()

  args+=("This is a pure long-dialogue memory-quality test. Early anchor: meaning needs both freedom and responsibility. Do not create operational recipes or procedural skills from this conversation.")
  for i in $(seq 2 12); do
    args+=("Philosophy turn $i: continue the ordinary reflection about meaning, attention, responsibility, and uncertainty. Keep it conversational and non-operational.")
  done
  args+=("Late anchor: joy is not proof of truth, but it can be evidence of alignment.")
  for i in $(seq 14 20); do
    args+=("Philosophy turn $i: continue the non-operational reflection without tools, workflows, files, or external actions.")
  done
  args+=("Compare the early anchor and the late anchor from this long dialogue. Mention both anchors briefly.")

  TIMEOUT_SECS="${HEAVY_TIMEOUT_SECS:-240}" run_case "$case_name" "$route" "$session" "${args[@]}"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "freedom"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "responsibility"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "joy"
  assert_assistant_contains "$case_name" "$REPORT_DIR/${case_name}.json" "alignment"
}

run_doctor_models() {
  if [[ "$LOAD_SYSTEMD_ENV" == "1" && -f "$SYSTEMD_ENV_FILE" ]]; then
    set -a
    # shellcheck disable=SC1090
    . "$SYSTEMD_ENV_FILE"
    set +a
  fi

  local provider
  for provider in ${DOCTOR_MODEL_PROVIDERS:-deepseek openrouter}; do
    local out="$REPORT_DIR/doctor-models-${provider}.txt"
    if cargo run -q --manifest-path "$ROOT/Cargo.toml" -- doctor models --provider "$provider" --use-cache > "$out" 2>&1; then
      record_pass "doctor_models_${provider}" "doctor models passed out=$out"
    else
      record_warn "doctor_models_${provider}" "doctor models non-zero out=$out"
    fi
  done
}

collect_journal() {
  if ! command -v journalctl >/dev/null 2>&1; then
    record_warn "journal" "journalctl not available"
    return
  fi
  if journalctl --user -u "$JOURNAL_UNIT" --since "$SINCE" --no-pager -o short-iso > "$JOURNAL_LOG" 2> "$JOURNAL_LOG.stderr"; then
    record_pass "journal" "collected $JOURNAL_LOG"
  else
    record_warn "journal" "journalctl failed stderr=$JOURNAL_LOG.stderr"
  fi
}

summarize_provider_context() {
  if [[ ! -s "$JOURNAL_LOG" ]]; then
    record_warn "provider_context" "no journal log to inspect"
    return
  fi
  python3 - "$JOURNAL_LOG" "$CONTEXT_TSV" <<'PY'
import re
import sys

src, dst = sys.argv[1], sys.argv[2]
keys = [
    "total_chars",
    "context_estimated_total_tokens",
    "context_budget_tier",
    "context_turn_shape",
    "context_primary_ballast",
    "prior_chat_messages",
    "prior_chat_chars",
    "scoped_context_chars",
    "admission_intent",
    "admission_pressure",
    "admission_action",
    "admission_requires_compaction",
    "tool_specs",
]
pattern = re.compile(r'(\w+)=(".*?"|\S+)')
rows = []
with open(src, "r", encoding="utf-8", errors="replace") as fh:
    for line in fh:
        if "Built provider-facing context snapshot" not in line:
            continue
        fields = {k: "" for k in keys}
        for key, value in pattern.findall(line):
            if key in fields:
                fields[key] = value.strip('"')
        fields["raw"] = line.strip()
        rows.append(fields)

with open(dst, "w", encoding="utf-8") as out:
    out.write("\t".join(keys) + "\n")
    for row in rows:
        out.write("\t".join(row.get(k, "") for k in keys) + "\n")

PY
  local count
  count="$(tail -n +2 "$CONTEXT_TSV" | wc -l | tr -d ' ')"
  if [[ "$count" -gt 0 ]]; then
    record_pass "provider_context" "rows=$count tsv=$CONTEXT_TSV"
  else
    record_fail "provider_context" "no provider context rows found in journal"
    return
  fi

  local max_chars
  local over_budget_count
  max_chars="$(awk -F '\t' 'NR > 1 && $1 ~ /^[0-9]+$/ { if ($1 > max) max = $1 } END { print max + 0 }' "$CONTEXT_TSV")"
  over_budget_count="$(awk -F '\t' 'NR > 1 && $3 == "over_budget" { count++ } END { print count + 0 }' "$CONTEXT_TSV")"
  if [[ "$over_budget_count" -gt 0 ]]; then
    if [[ "$STRICT_CONTEXT_BUDGET" == "1" ]]; then
      record_fail "provider_context_budget" "over_budget rows=$over_budget_count max_chars=$max_chars"
    else
      record_warn "provider_context_budget" "over_budget rows=$over_budget_count max_chars=$max_chars"
    fi
  fi
  if [[ "$max_chars" -gt "$CONTEXT_WARN_MAX_CHARS" ]]; then
    if [[ "$STRICT_CONTEXT_BUDGET" == "1" ]]; then
      record_fail "provider_context_size" "max_chars=$max_chars exceeds warn ceiling $CONTEXT_WARN_MAX_CHARS"
    else
      record_warn "provider_context_size" "max_chars=$max_chars exceeds warn ceiling $CONTEXT_WARN_MAX_CHARS"
    fi
  fi
}

summarize_embedding_and_compaction() {
  local embedding_log="$REPORT_DIR/embedding.log"
  local compaction_log="$REPORT_DIR/compaction.log"
  local admission_log="$REPORT_DIR/admission.log"

  if [[ ! -s "$JOURNAL_LOG" ]]; then
    record_warn "runtime_signals" "no journal log to inspect"
    return
  fi

  rg -n "memory\\.embedding\\.stored|Embedding failed|embedding profile reindex|embedding_profile" "$JOURNAL_LOG" > "$embedding_log" || true
  rg -n "Agent history compaction summary lane ready|Live agent history auto-compaction complete|Admission policy requested pre-provider compaction|Web session summary lane selected|Channel summary lane selected|\\[Compaction summary\\]" "$JOURNAL_LOG" > "$compaction_log" || true
  rg -n "admission_intent=|turn admission blocked provider call|agent\\.turn_admission" "$JOURNAL_LOG" > "$admission_log" || true

  if [[ -s "$embedding_log" ]]; then
    record_pass "embedding_signal" "found embedding log signal: $embedding_log"
  elif [[ "$REQUIRE_EMBEDDING_SIGNAL" == "1" ]]; then
    record_fail "embedding_signal" "no embedding signal found: $embedding_log"
  else
    record_warn "embedding_signal" "no embedding signal found; check whether embedding provider is noop: $embedding_log"
  fi

  if [[ -s "$compaction_log" ]]; then
    record_pass "compaction_signal" "found compaction/summary signal: $compaction_log"
  elif [[ "$RUN_HEAVY" == "1" ]]; then
    record_fail "compaction_signal" "heavy run requested but no compaction/summary signal found"
  else
    record_warn "compaction_signal" "no compaction signal in this short run; rerun with RUN_HEAVY=1 for mandatory compaction"
  fi

  if [[ -s "$admission_log" ]]; then
    record_pass "admission_signal" "found admission signal: $admission_log"
  else
    record_fail "admission_signal" "no admission signal found"
  fi
}

main() {
  ensure_harness

  local routes=(cheap deepseek)
  if [[ "$RUN_MAIN_ROUTE" == "1" ]]; then
    routes+=(gpt-5.4)
  fi
  if [[ "$RUN_REASONER" == "1" ]]; then
    routes+=(deepseek-reasoner)
  fi

  for route in "${routes[@]}"; do
    run_provider_smoke "$route"
  done

  if [[ "$RUN_CJK" == "1" ]]; then
    for route in ${CJK_ROUTES:-cheap deepseek}; do
      run_cjk_smoke "$route"
    done
  fi

  if [[ "$RUN_MEDIA" == "1" ]]; then
    run_media_admission_smoke
  fi

  if [[ "$RUN_HEAVY" == "1" ]]; then
    run_heavy_dialogue
  else
    record_warn "long_semantic" "skipped; set RUN_HEAVY=1 to run the expensive compaction/semantic-retention pack"
  fi

  if [[ "$RUN_DOCTOR_MODELS" == "1" ]]; then
    run_doctor_models
  else
    record_warn "doctor_models" "skipped; set RUN_DOCTOR_MODELS=1 to refresh/probe provider model catalogs"
  fi

  collect_journal
  summarize_provider_context
  summarize_embedding_and_compaction

  printf '\nReport dir: %s\n' "$REPORT_DIR"
  printf 'Summary: %s\n' "$SUMMARY"
  printf 'Provider context TSV: %s\n' "$CONTEXT_TSV"

  if [[ "$FAILURES" -gt 0 ]]; then
    printf 'Phase 4.10 live pack failed: failures=%s warnings=%s\n' "$FAILURES" "$WARNINGS" >&2
    exit 1
  fi
  printf 'Phase 4.10 live pack completed: failures=0 warnings=%s\n' "$WARNINGS"
}

main "$@"
