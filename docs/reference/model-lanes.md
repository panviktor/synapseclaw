# Model Lanes Reference

Model lanes route non-primary model work without changing the main chat model. The same `[[model_lanes]]` mechanism is used for compaction, embeddings, cheap reasoning, web extraction, tool validation, multimodal understanding, speech transcription, speech synthesis, and media generation.

## Why Lanes Exist

The primary model should stay focused on the user turn. Auxiliary work often needs different tradeoffs: compaction should be cheap and reliable, embeddings need vector dimensions, voice input/output uses direct speech APIs, image or audio generation needs capability-specific models, and tool validators should be isolated from normal chat routing.

Older keys such as `summary_model`, `[summary].provider`, `embedding_routes`, and `[memory].embedding_*` are not supported. Configure auxiliary models through `[[model_lanes]]` only.

## Basic Shape

```toml
model_preset = "chatgpt"

[[model_lanes]]
lane = "compaction"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3.6-plus"
api_key_env = "OPENROUTER_API_KEY"

[[model_lanes]]
lane = "embedding"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3-embedding-8b"
api_key_env = "OPENROUTER_API_KEY"
dimensions = 4096

[model_lanes.candidates.profile]
features = ["embedding"]
```

Candidates are ordered. The runtime records the candidate order and selected candidate in bounded diagnostics, but it does not put the full lane config into every provider prompt.

## Common Config Examples

### Two Primary Models: ChatGPT And Anthropic

Use this when you want ChatGPT as the normal default, but Anthropic available as the next main-reasoning candidate if the primary provider is unavailable. Keep the current default provider/model as the first `reasoning` candidate; normal turns stay on that model, and failover only moves on typed provider failures such as quota, payment, hard rate limit, timeout, connection, or server errors.

```toml
default_provider = "openai-codex"
default_model = "gpt-5.4"
model_preset = "chatgpt"

[[model_lanes]]
lane = "reasoning"

[[model_lanes.candidates]]
provider = "openai-codex"
model = "gpt-5.4"
api_key_env = "OPENAI_API_KEY"

[[model_lanes.candidates]]
provider = "anthropic"
model = "claude-sonnet-4-6"
api_key_env = "ANTHROPIC_API_KEY"
```

### Two Models: Strong Chat, Cheap Compaction

Use this when the main conversation should stay on a strong model, but routine history compression should run on a cheaper model. If you also need vector memory or media features, use the multi-feature example below.

```toml
default_provider = "openai-codex"
default_model = "gpt-5.4"
model_preset = "chatgpt"

[[model_lanes]]
lane = "compaction"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3.6-plus"
api_key_env = "OPENROUTER_API_KEY"
```

### Several Models By Feature

Use this when different subsystems need different capabilities. The primary model handles normal reasoning, `compaction` handles summaries, `embedding` handles vector recall, and media lanes are selected only by features that need them.

```toml
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4-6"
model_preset = "openrouter"

[[model_lanes]]
lane = "compaction"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3.6-plus"
api_key_env = "OPENROUTER_API_KEY"

[[model_lanes]]
lane = "embedding"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3-embedding-8b"
api_key_env = "OPENROUTER_API_KEY"
dimensions = 4096

[model_lanes.candidates.profile]
features = ["embedding"]

[[model_lanes]]
lane = "image_generation"

[[model_lanes.candidates]]
provider = "openrouter"
model = "google/gemini-3.1-flash-image-preview"
api_key_env = "OPENROUTER_API_KEY"

[[model_lanes]]
lane = "tool_validator"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3.6-plus"
api_key_env = "OPENROUTER_API_KEY"
```

### Ordered Candidates For One Lane

Use multiple candidates when several models can serve the same lane and you want a clear operator-reviewed order. Selection validates lane capabilities before provider calls; it is not a hidden return to an unrelated model.

```toml
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4-6"
model_preset = "openrouter"

[[model_lanes]]
lane = "compaction"

[[model_lanes.candidates]]
provider = "openrouter"
model = "deepseek/deepseek-v4"
api_key_env = "OPENROUTER_API_KEY"

[[model_lanes.candidates]]
provider = "openrouter"
model = "qwen/qwen3.6-plus"
api_key_env = "OPENROUTER_API_KEY"
```

For `reasoning`, `compaction`, `embedding`, and `speech_synthesis`, ordered candidates are also the failover order. Payment errors, exhausted quota, hard rate limits, connection failures, timeouts, and server failures can move to the next candidate; context-window overflow does not, because another provider should not hide a prompt budget bug.

The primary conversation path uses the `reasoning` failover chain only when the first candidate is exactly the configured `default_provider` and `default_model`. That keeps accidental lane edits from silently changing the normal chat model.

Embedding failover only uses candidates with the same vector dimensions as the selected candidate. This prevents one outage from silently mixing incompatible vector sizes in the same memory store.

### Voice Notes: STT And TTS

Use this when Matrix, Telegram, or WhatsApp voice messages should be transcribed, or when a voice-capable channel should speak replies back. The lane picks the speech provider and model; `[transcription]` and `[tts]` only keep voice-specific knobs such as language, voice id, format, duration, and text length.

```toml
[transcription]
enabled = true
max_duration_secs = 120

[[model_lanes]]
lane = "speech_transcription"

[[model_lanes.candidates]]
provider = "groq"
model = "whisper-large-v3-turbo"
api_key_env = "GROQ_API_KEY"

[tts]
enabled = true
default_format = "mp3"
max_text_length = 4096

[[model_lanes]]
lane = "speech_synthesis"

[[model_lanes.candidates]]
provider = "xai"
model = "tts"
api_key_env = "XAI_API_KEY"
```

Voice IDs come from the runtime voice catalog exposed by `voice_list`. For provider-specific voice defaults, keep small blocks under `[tts.<provider>]`. For example, MiniMax keeps voice tuning under `[tts.minimax]`, while the model and key are selected through `speech_synthesis`.

```toml
[tts.minimax]
voice_id = "<voice-id-from-voice-list-or-provider-catalog>"
speed = 1.0
volume = 1.0
pitch = 0

[[model_lanes]]
lane = "speech_synthesis"

[[model_lanes.candidates]]
provider = "minimax"
model = "speech-2.8-hd"
api_key_env = "MINIMAX_API_KEY"
```

## Secrets On Linux

On Linux with the systemd user service, keep provider keys in a local environment file such as `~/.config/systemd/user/synapseclaw.env`. Reference those keys from config with `api_key_env`; do not put raw API keys in tracked repository files.

```bash
OPENAI_API_KEY=...
ANTHROPIC_API_KEY=...
OPENROUTER_API_KEY=...
GROQ_API_KEY=...
MISTRAL_API_KEY=...
MINIMAX_API_KEY=...
XAI_API_KEY=...
```

After changing the environment file, restart the user service so systemd reloads the variables.

## Supported Lane Names

- `reasoning`
- `cheap_reasoning`
- `compaction`
- `embedding`
- `web_extraction`
- `tool_validator`
- `speech_transcription`
- `speech_synthesis`
- `multimodal_understanding`
- `image_generation`
- `audio_generation`
- `video_generation`
- `music_generation`

`reasoning` is the primary task lane. The other lanes are auxiliary lanes and should be used when a subsystem needs a specialized model.

## Compaction

Compaction uses the `compaction` lane for rolling session summaries and history compression. If the lane is missing, compaction that needs a summary is skipped loudly instead of silently using the primary model.

`[summary]` still exists only for summary tuning such as `temperature`. It no longer selects provider, model, or API key.

```toml
[summary]
temperature = 0.3
```

## Embeddings

Embeddings use the `embedding` lane. The selected candidate must have a positive `dimensions` value; otherwise vector embeddings are disabled and the memory backend falls back to non-vector behavior.

Use provider-specific model ids exactly as the provider expects them. Keep secrets in env vars such as `OPENROUTER_API_KEY`, not in tracked config files.

## Presets And Overrides

A preset may provide bundled auxiliary lanes. Explicit `[[model_lanes]]` entries override the preset lane with the same name.

Use explicit lanes for production fleets. That makes compaction, embedding, and media routing reviewable and keeps web/channel behavior aligned.

## Voice I/O

The `speech_transcription` lane selects the STT provider/model for channel voice notes. Supported direct adapters include Groq Whisper, OpenAI Whisper/transcribe models, Deepgram, AssemblyAI, Google STT, and Mistral Voxtral Transcribe.

The `speech_synthesis` lane selects the TTS provider/model for spoken replies. Supported direct adapters include OpenAI TTS, ElevenLabs, Google Cloud TTS, Edge TTS, MiniMax Speech, Mistral Voxtral TTS, and xAI TTS.

The `audio_generation` lane is different: it is for model-generated audio as an agent output, not for channel voice-note transcription or reply playback. Live voice calls, Discord voice channels, LiveKit/WebRTC, and video calls require a streaming channel/runtime layer on top of these lanes; they are not enabled just by selecting a TTS model.

## Operator Checks

Run:

```bash
synapseclaw doctor
```

Doctor reports selected core auxiliary lanes and any configured optional lanes. A healthy config should show `compaction` and `embedding` selections when those subsystems are expected to run.

Runtime logs also emit compact lane decisions:

- `Embedding auxiliary lane selected`
- `Inbound session summary auxiliary lane selected`
- `Agent history compaction auxiliary lane ready`
- `Speech transcription lane selected`
- `Speech synthesis lane candidates resolved`

## Removed Config

These keys are rejected during config load:

- `summary_model`
- `summary.provider`
- `summary.model`
- `summary.api_key_env`
- `embedding_routes`
- `memory.embedding_provider`
- `memory.embedding_model`
- `memory.embedding_dimensions`

Replace them with `[[model_lanes]]`.
