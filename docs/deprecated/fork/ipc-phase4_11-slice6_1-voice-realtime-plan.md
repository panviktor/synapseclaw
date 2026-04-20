# Phase 4.11 Slice 6.1: Voice And Realtime Media Runtime Plan

This is an internal implementation plan, not user-facing documentation.
Slice 6 unified auxiliary model lanes for STT, TTS, and media model selection.
Slice 6.1 adds the missing runtime layer above those lanes: persistent voice
preferences, channel-aware auto-TTS policy, streaming spoken replies, CLI voice
mode, realtime call foundations, and media delivery diagnostics.

Hermes is the implementation reference for practical voice mode: push-to-talk,
silence auto-stop, local playback, gateway slash commands, Discord voice-channel
loop, and simple platform-specific media delivery. OpenClaw remains the policy
reference: explicit TTS modes, provider/status commands, failover visibility,
and call lifecycle discipline.

## Goals

- Make voice choice durable without hardcoded prompt text.
- Let different channels and conversations use different voices.
- Keep spoken replies controlled by typed policy, not model prose.
- Preserve one shared media delivery decision path for web and channels.
- Add a realtime voice runtime that can grow into Discord voice, phone calls,
  LiveKit/WebRTC, Telegram calls, Signal calls, Matrix calls, and video calls.
- Keep offline `audio_generation` and `video_generation` separate from realtime
  audio/video sessions.

## 6.1A: Preferences And Policy

Add a domain-level voice preference system with `global`, `channel`, and
`conversation` scopes. Resolution order is fixed: explicit `voice_reply` argument,
conversation preference, channel preference, global preference, then
`[tts].default_voice`.

Add typed controls for voice settings instead of prompt directives. The agent,
CLI, and gateway must be able to get, set, clear, and list voice preferences;
requested provider, model, voice, and format are validated against `voice_list`
and lane candidates before saving.

Add structured auto-TTS policy. Supported policies are `inherit`, `off`,
`always`, `inbound_voice`, `tagged`, `channel_default`, and
`conversation_default`.
The default remains conservative: no automatic spoken reply unless enabled by
operator or scoped preference.

Status: implemented for CLI, gateway API, `voice_preference` tool, manual
`voice_reply`, and shared channel response auto-TTS policy. Web UI controls are
still pending.

## 6.1B: Auto-TTS And Streaming

Apply voice policy in shared channel and web reply paths. A normal text answer
must not become audio unless policy allows it, and a voice-triggered answer must
use the same resolver as manual `voice_reply`.

Add streaming TTS v1 as sentence-buffer streaming. The runtime splits stable
assistant text into chunks, synthesizes chunks sequentially, plays or delivers
them as they become ready, and supports cancellation. Native provider streaming
can later sit behind the same interface.

Status: policy-triggered full-response auto-TTS is implemented for channel
responses. Sentence-buffer streaming and cancellation-aware chunk delivery are
pending.

## 6.1C: CLI Voice Mode

Add Hermes-style CLI voice mode: push-to-talk recording, silence auto-stop,
STT submission, local playback, `/voice on|off|tts|status`, and a headless-safe
environment doctor. STT uses `speech_transcription`; spoken replies use
`speech_synthesis`.

The CLI mode must degrade cleanly over SSH, headless systemd sessions, Docker,
and machines without audio devices. It should explain the missing capability
without crashing the main agent.

## 6.1D: Realtime Call Runtime

Introduce a provider-neutral call session runtime with states: `created`,
`ringing`, `connected`, `listening`, `thinking`, `speaking`, `ended`, and
`failed`. The first concrete adapter should build on the current ClawdTalk and
Telnyx direction; Twilio and LiveKit are later adapters behind the same trait.

The call runtime owns VAD/speech segment boundaries, STT, agent turn dispatch,
TTS playback, hangup, timeout, stale session cleanup, and diagnostics. Video
calls must attach to this session runtime later; they are not the same thing as
the offline `video_generation` lane.

## 6.1E: Chat-App Audio And Video Calls

Telegram, Signal, and Matrix are first-class realtime call targets, not just
voice-note transports. Their support should be planned as adapters over the same
call session runtime, with each adapter responsible only for protocol-specific
signaling, media transport, permissions, and client limitations.

Telegram calls require a protocol feasibility pass before implementation because
Bot API support for user calls is not equivalent to normal message delivery.
If native bot calls are not available for the deployed path, the runtime should
support an explicit bridge/provider adapter rather than pretending voice notes
are calls.

Signal calls require a feasibility pass around the local Signal bridge actually
used by our deployment. If the bridge exposes only messaging and attachments,
Signal remains voice-note capable until a real call bridge exists.

Matrix calls should target MatrixRTC/WebRTC-style signaling where available,
while preserving MSC3245 voice-note delivery as a separate message feature.
Matrix call support must be tested against Element Web/Desktop/iOS separately,
because client behavior differs.

## 6.1F: Discord Voice And Video-Ready Shape

Add Discord voice-channel support after the generic call runtime shape is in
place. The target behavior mirrors Hermes: join a voice channel, receive Opus
frames, detect speech segments, transcribe, run the agent turn, speak back, and
leave or report status on command.

Define LiveKit/WebRTC extension points, but do not fake video calls with file
generation. `video_generation` remains an async media-output lane; realtime video
is a session transport.

## 6.1G: Web UI And User Docs

Expose operator controls for voice status, preferences, policies, diagnostics,
and active sessions. The web UI must use the same gateway APIs as channel/runtime
paths and must not create a separate preference mechanism.

User documentation should not mention slice numbers. It should explain the three
voice modes separately: voice notes in chat, local CLI voice mode, and realtime
audio/video sessions. Provider setup belongs in model lanes docs; everyday usage
belongs in simple voice docs.

## Public Interfaces

CLI:

```bash
synapseclaw voice status
synapseclaw voice profiles --json
synapseclaw voice voices --json
synapseclaw voice synthesize --text "Voice test." --voice hannah --output /tmp/voice-test.wav
synapseclaw voice transcribe --file /tmp/voice-test.wav
synapseclaw voice preference get|set|clear --scope global|channel|conversation ...
synapseclaw voice mode on|off|status
```

Gateway API:

- `GET /api/voice/status`
- `GET /api/voice/profiles`
- `GET /api/voice/voices`
- `POST /api/voice/synthesize`
- `GET /api/voice/preferences`
- `POST /api/voice/preferences`
- `DELETE /api/voice/preferences`
- `GET /api/voice/policies`
- `POST /api/voice/policies`
- `DELETE /api/voice/policies`
- later in this slice: `POST /api/voice/sessions`
- later in this slice: `GET /api/voice/sessions/{id}`
- later in this slice: `POST /api/voice/sessions/{id}/end`

Agent tools:

- `voice_list` returns voices, delivery profiles, current resolved preference,
  and current policy.
- `voice_reply` resolves scoped preference automatically when no explicit voice
  is provided.
- `voice_preference` changes durable voice/provider/format preference.
- `voice_policy` changes scoped auto spoken-reply behavior.

Storage:

- Voice preferences are durable private operational state.
- Voice preferences are excluded from replay until typed privacy classification
  explicitly allows otherwise.
- Raw API keys and transient audio payloads are never stored in preference
  records.

## Acceptance Tests

- Preference precedence is explicit voice, conversation, channel, global, config.
- Invalid provider, model, voice, or format is rejected before persistence.
- Clearing a scoped preference falls back to the next available scope.
- Auto-TTS policy allows or blocks spoken replies deterministically.
- Matrix and Telegram can have different default voices at the same time.
- Conversation preference overrides channel preference only in that conversation.
- `voice_reply` uses a newly saved preference without daemon restart.
- Streaming TTS emits chunks in order and can be cancelled.
- CLI voice mode can run a mocked record, STT, agent, TTS playback flow.
- Call state machine rejects impossible transitions.
- Realtime adapter handles connect, speech segment, agent reply, hangup, and
  timeout in tests.
- Telegram, Signal, and Matrix call support each has a feasibility report before
  implementation.
- Matrix call tests distinguish voice note delivery, audio call, and video call.

## Assumptions

- No OpenClaw-style `[[tts:...]]` prompt directives. Voice changes use typed
  tools and APIs.
- No unsafe local audio conversion by default. Provider-native valid Opus is
  preferred where a channel needs native voice-note behavior.
- Hermes is the reference for CLI and Discord voice runtime.
- OpenClaw is the reference for policy and diagnostics.
- Phone calls start with the existing ClawdTalk and Telnyx direction.
- Telegram, Signal, and Matrix calls require protocol feasibility checks before
  code claims support.
- Video calls are planned as realtime session extensions, not normal media
  generation.
