# Realtime Calls

Realtime calls are separate from voice notes and speech synthesis. A voice note is an audio artifact sent through a chat channel; a realtime call is a live session with start, answer, speak, and hangup side effects.

As of the current `6.1` implementation, the most complete live-call path is Matrix audio calls. ClawdTalk remains available as a typed call runtime, but Matrix is the path that currently has the full `speech in -> transcript -> LLM -> TTS -> speech out` loop plus `chat -> call -> chat` handoff.

## Capability Model

Channels declare call support through the shared capability profile. `available` means the adapter has an executable runtime path, `control_only` means the transport already exposes typed call-control and inspection signals but still lacks a bot-side media participant, `planned` means the roadmap tracks the channel but execution must not rely on it yet, and `unsupported` means there is no current runtime plan.

Inspect the matrix with:

```bash
synapseclaw voice profiles --json
```

The same information is exposed through:

```text
GET /api/voice/profiles
GET /api/channels/capabilities
```

## Current Runtime

`clawdtalk` exposes a typed telephony runtime with `start`, `answer`, `speak`, and `hangup`.

`matrix` exposes a typed MatrixRTC runtime too. It can start a room call ring, track inbound and outbound call-control events, answer by attaching to the MatrixRTC/LiveKit media session, decrypt remote audio, stream it into the configured realtime speech provider, publish synthesized audio back into the call, and send a typed hangup or decline event.

Call sessions use a shared state vocabulary: `created`, `ringing`, `connected`, `listening`, `thinking`, `speaking`, `ended`, and `failed`. Adapters must move through valid transitions, so future Matrix, Telegram, Signal, Discord, or WebRTC integrations report status in the same shape instead of inventing per-channel state strings.

The current Matrix runtime uses those states directly too: remote caller speech moves the session to `listening`, the agent turn moves it to `thinking`, and synthesized playback moves it to `speaking`.

Outbound calls can also carry a generic call plan. The shared contract supports a free-form `objective`, optional `context`, and optional `agenda`, so the same runtime can handle a morning briefing, a restaurant booking, a stock check, or another call task without baking product behavior into an enum.

The call ledger also keeps trigger provenance. A call may start from a chat request, a scheduled job, the CLI, the gateway API, or an inbound transport event, and later session inspection surfaces that origin without changing the runtime behavior.

The shared call ledger is persisted under `~/.synapseclaw/state/realtime-call-sessions.json`. That means `voice call start`, `voice_call.start`, gateway `POST /api/voice/calls/start`, and inbound transport updates can be inspected later from a separate CLI/API/tool process through the same `sessions` and `get` surfaces.

The shared call ledger now also applies idle cleanup for non-terminal sessions. If a realtime call stops updating for too long, status and session reads mark it terminal instead of leaving a permanently active `listening` or `thinking` call in the ledger.

SynapseClaw supports two explicit ClawdTalk paths:

- Telnyx call-control actions for outbound `start`, `speak`, and `hangup`.
- A ClawdTalk outbound WebSocket bridge for the current text-in/text-out call loop.

The public ClawdTalk documentation at <https://www.clawdtalk.com/> describes a persistent outbound WebSocket connection where the bot receives transcript events such as `{ "event": "message", "call_id": "...", "text": "..." }` and responds with `{ "type": "response", "call_id": "...", "text": "..." }`. SynapseClaw's bridge follows that JSON shape and feeds transcript turns into the same channel pipeline used by other adapters.

Configure the bridge explicitly:

```toml
[channels_config.clawdtalk]
api_key = "${CLAWDTALK_API_KEY}"
websocket_url = "https://clawdtalk.com"
api_base_url = "https://clawdtalk.com/v1"
assistant_id = "your-assistant-id"
connection_id = "telnyx-connection-id"
from_number = "+15551234567"
allowed_destinations = ["+1555"]
```

`https://` endpoints are normalized to `wss://` for the outbound socket. If `api_base_url` is omitted, SynapseClaw derives `https://.../v1` from `websocket_url` for outbound `POST /calls`. Without `websocket_url`, the ClawdTalk channel does not start the transcript bridge.

## Matrix Setup

For Matrix live calls, four things must be configured together:

1. a working Matrix channel login
2. a `speech_transcription` path that can handle live call audio
3. a `speech_synthesis` path for call replies
4. a live-call runtime policy under `[agent.live_calls]`

Keep secrets in `~/.config/systemd/user/synapseclaw.env`, not in tracked config files.

Minimal example:

```toml
[channels_config.matrix]
homeserver_url = "https://matrix.example.com"
bot_user_id = "@openclaw:example.com"
access_token = "${MATRIX_ACCESS_TOKEN}"
room_id = "!ops:example.com"

[transcription]
enabled = true
default_provider = "deepgram"
model = "nova-3"

[transcription.deepgram]
api_key = "${DEEPGRAM_API_KEY}"
model = "nova-3"

[transcription.deepgram.flux]
enabled = true
model = "flux-general-multi"
sample_rate_hz = 48000

[tts]
enabled = true
default_provider = "openai"
default_voice = "alloy"
default_format = "wav"

[tts.openai]
api_key = "${OPENAI_API_KEY}"
model = "gpt-4o-mini-tts"

[agent.live_calls]
provider = "openai-codex"
model = "gpt-5.4-mini"
max_tool_iterations = 2
max_spoken_chars = 220
max_spoken_sentences = 2
excluded_tools = ["memory_recall", "session_search", "shell"]
profile_locale_key = "response_locale"
fallback_locale = "en"

[agent.live_calls.greetings]
en = "Hello. I'm here."
ru = "Привет. Я на связи."
```

Operational notes:

- `speech_transcription` is what hears the caller.
- `transcription.deepgram.flux` is what owns turn boundaries for live calls.
- `speech_synthesis` is what the bot speaks back into the call.
- `[agent.live_calls]` controls the live-call model, reply budget, excluded tools, locale fallback, and per-locale greeting.
- Current production expectation is audio calls, not video calls.

## CLI

Realtime call commands require explicit confirmation because they create external telephony side effects.

```bash
synapseclaw voice call status --json
synapseclaw voice call sessions --json
synapseclaw voice call get --call-control-id call_123 --json
synapseclaw voice call start --to +15551234567 --confirm
synapseclaw voice call start --channel clawdtalk --to +15551234567 --confirm
synapseclaw voice call start --channel matrix --to @user:example.com --objective "Ring the user for a short morning briefing." --confirm
synapseclaw voice call start --channel matrix --to !roomid:example.com --objective "Ring this Matrix room." --confirm
synapseclaw voice call start --channel matrix --to '#ops:example.com' --objective "Ring the aliased Matrix room." --confirm
synapseclaw voice call start --to +15551234567 --objective "Call the restaurant and reserve a table for two at 19:00." --context "Prefer a quiet place near Alexanderplatz." --agenda "Ask whether they have availability at 19:00" --agenda "Confirm the reservation details" --confirm
synapseclaw voice call answer --channel clawdtalk --call-control-id call_123 --confirm
synapseclaw voice call speak --call-control-id call_123 --text "Hello from SynapseClaw." --confirm
synapseclaw voice call hangup --call-control-id call_123 --confirm
```

If `--confirm` is omitted, the CLI exits before contacting the call provider.

If exactly one realtime call transport is configured, CLI, tool, and gateway paths select it automatically. If several transports become available at once, `channel` / `--channel` becomes mandatory for side-effect actions.

For `clawdtalk`, `answer` means "attach to or resume handling an inbound call session that is already established by the transport". It does not claim a separate provider-side accept step that the current websocket runtime does not expose.

For `matrix`, `answer` means "attach the bot to the MatrixRTC media session for this call", and `speak` publishes a synthesized speech segment into that attached LiveKit room.

Matrix `start` accepts four target shapes:

- `room` to use the configured base room
- `!room:id` to ring an explicit room id
- `#alias:server` to resolve and ring a room alias
- `@user:server` to reuse or create a direct-message room for that user, then ring there

When a Matrix call starts in a DM room, later `hangup` resolves that same room from the shared call ledger instead of falling back to the base room.

Inbound Matrix ring events and live-call transcript turns enter the shared runtime as typed call-related events tied to the same call session. SynapseClaw accepts both the older `m.rtc.notification` ring path and the newer `org.matrix.msc3401.call.member` membership path used by Element and MatrixRTC clients.

`voice call status` reports the shared transport registry: which call transports are configured, which runtimes are available versus control-only versus only planned, whether media is really attached, which runtime would be selected by default, which typed actions are supported (`start`, `answer`, `speak`, `hangup`, `inspect`), and any typed runtime health currently exposed by an adapter. It performs a read-only health probe for configured runtimes before printing the result, exposes transport-specific readiness details such as Matrix configured auth mode, effective auth source, and room access or ClawdTalk outbound/call-control readiness, and intentionally does not store or print transcript text.

For MatrixRTC bootstrap, SynapseClaw supports both common deployment shapes through one path: the newer homeserver-advertised `/_matrix/client/v1/rtc/transports` plus `/get_token`, and the older `.well-known` or focus discovery plus `/sfu/get`. Status distinguishes simple route discovery from a real authorizer grant exchange, so `media_bootstrap_ready=true` means SynapseClaw successfully resolved the focus, obtained an OpenID token, and fetched a LiveKit JWT from the deployment-specific authorizer path.

For `clawdtalk`, `runtime_ready` means SynapseClaw has enough outbound call configuration to execute call actions, not merely that a `[channels_config.clawdtalk]` section exists.

`voice call sessions` and `voice call get` read the shared persisted session ledger. They are read-only inspection commands and do not require `--confirm`.

## Chat Tool

When a realtime call transport is configured, the agent also receives the `voice_call` tool. It uses the same typed runtime as the CLI and gateway, so a chat request such as "call me now" does not go through a separate prompt-only command parser. The runtime records that launch as `chat_request`, which later lets the system distinguish it from scheduled or operator-started calls.

## Current Matrix Behavior

What currently works:

- inbound and outbound Matrix audio calls
- MatrixRTC media bootstrap on both newer and older homeserver layouts
- encrypted media key exchange and remote audio decrypt
- realtime transcript turns from the call
- short voice replies back into the same call
- `chat -> call` handoff and `call -> chat` recap
- live-call policy from `[agent.live_calls]`, including model selection, locale fallback, tool budget, and greeting text

What is still a known limitation:

- long multi-turn calls are usable but still not as fast as they should be
- voice runtime latency is the main remaining quality gap
- language and voice selection are policy-driven, but still worth validating per deployment

`voice_call.status`, `voice_call.list_sessions`, and `voice_call.get_session` are read-only inspection actions. `start`, `answer`, `speak`, and `hangup` must pass `confirm: true`; without that explicit confirmation the tool returns before provider access.

For `start`, the typed tool and gateway accept optional structured call-plan fields:

- `objective`
- `context`
- `agenda`

When those are present, SynapseClaw builds a bounded opening prompt for the call runtime instead of relying only on one raw free-form `prompt` string.

## Gateway API

The gateway exposes the same runtime operations:

```text
GET /api/voice/calls/status
GET /api/voice/calls/sessions
GET /api/voice/calls/sessions/{call_control_id}
POST /api/voice/calls/start
POST /api/voice/calls/answer
POST /api/voice/calls/speak
POST /api/voice/calls/hangup
```

Every request must include `confirm: true`. Without confirmation the handler rejects the request before provider access.

Status:

```json
{
  "status": "ok",
  "report": {
    "default_channel": "clawdtalk",
    "channels": [
      {
        "channel": "clawdtalk",
        "transport_configured": true,
        "audio_call_runtime": "available",
        "video_call_runtime": "planned",
        "media_attached": true,
        "action_support": {
          "start": true,
          "answer": true,
          "speak": true,
          "hangup": true,
          "inspect": true
        },
        "runtime_selected_by_default": true,
        "runtime_ready": true,
        "health": {
          "ready": true,
          "connected": true,
          "reconnect_attempts": 1,
          "active_calls": [
            {
              "call_control_id": "clk_123",
              "state": "listening",
              "message_count": 2
            }
          ]
        }
      },
      {
        "channel": "matrix",
        "transport_configured": true,
        "audio_call_runtime": "available",
        "video_call_runtime": "planned",
        "media_attached": false,
        "action_support": {
          "start": true,
          "answer": true,
          "speak": true,
          "hangup": true,
          "inspect": true
        },
        "runtime_selected_by_default": false,
        "runtime_ready": true,
        "health": {
          "ready": true,
          "recent_sessions": []
        }
      }
    ]
  }
}
```

Sessions:

```json
{
  "status": "ok",
  "channel": "clawdtalk",
  "sessions": [
    {
      "call_control_id": "clk_123",
      "direction": "inbound",
      "state": "listening",
      "message_count": 2
    }
  ]
}
```

Start:

```json
{
  "channel": "clawdtalk",
  "to": "+15551234567",
  "objective": "Call the restaurant and reserve a table for two at 19:00.",
  "context": "Prefer a quiet place near Alexanderplatz.",
  "agenda": [
    "Ask whether they have availability at 19:00",
    "Confirm the reservation details"
  ],
  "confirm": true
}
```

Answer:

```json
{
  "channel": "clawdtalk",
  "call_control_id": "call_123",
  "confirm": true
}
```

Speak:

```json
{
  "channel": "clawdtalk",
  "call_control_id": "call_123",
  "text": "Hello from SynapseClaw.",
  "confirm": true
}
```

Hangup:

```json
{
  "channel": "clawdtalk",
  "call_control_id": "call_123",
  "confirm": true
}
```

## Planned Channels

Matrix now exposes a real typed audio-call layer when the Matrix channel is configured: SynapseClaw tracks room readiness, inbound `m.rtc.notification`, inbound `org.matrix.msc3401.call.member`, decline/end events, recent call sessions, outbound ring / decline events, MatrixRTC bootstrap health, LiveKit media attach, and one-shot synthesized audio publish through the same shared runtime contract used by ClawdTalk.

What is still missing on `matrix` is the reverse path: remote live speech inside the call is not yet transcribed back into agent turns. Telegram audio calls and Signal audio calls are still marked as planned only.

For Matrix specifically, the current SynapseClaw build uses `matrix-sdk` `0.16` with `e2e-encryption`, `rustls-tls`, `markdown`, `sqlite`, and the widget-related `experimental-widgets` path. That is enough for encrypted messaging, media transfer, replies, reactions, MSC3245-style voice-note delivery, and access to the upstream widget driver and Element Call configuration types, but it is not a MatrixRTC media runtime by itself.

The upstream Rust SDK also exposes call-adjacent helpers such as decline-call events and room-call presence metadata. SynapseClaw now uses that control plane together with a LiveKit media attach path, so the next Matrix milestone is no longer "join the room somehow" but "transcribe inbound remote audio and keep the session conversational".

Video calls are also planned for ClawdTalk, but only audio calls are currently available.
