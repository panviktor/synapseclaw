# Model Lanes Reference

Model lanes route non-primary model work without changing the main chat model. The same `[[model_lanes]]` mechanism is used for compaction, embeddings, cheap reasoning, web extraction, tool validation, multimodal understanding, and media generation.

## Why Lanes Exist

The primary model should stay focused on the user turn. Auxiliary work often needs different tradeoffs: compaction should be cheap and reliable, embeddings need vector dimensions, image or audio generation need capability-specific models, and tool validators should be isolated from normal chat routing.

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

Use this when you want ChatGPT as the normal default, but Anthropic available as an explicit reasoning candidate for operator-selected routes and future policy routing. Keep the current default as the first `reasoning` candidate if you want normal turns to stay on that model.

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

Use multiple candidates when several models can serve the same lane and you want a clear operator-reviewed order. Selection validates lane capabilities before provider calls; it is not a hidden return to the primary model.

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

## Secrets On Linux

On Linux with the systemd user service, keep provider keys in a local environment file such as `~/.config/systemd/user/synapseclaw.env`. Reference those keys from config with `api_key_env`; do not put raw API keys in tracked repository files.

```bash
OPENAI_API_KEY=...
ANTHROPIC_API_KEY=...
OPENROUTER_API_KEY=...
```

After changing the environment file, restart the user service so systemd reloads the variables.

## Supported Lane Names

- `reasoning`
- `cheap_reasoning`
- `compaction`
- `embedding`
- `web_extraction`
- `tool_validator`
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

The `audio_generation` lane is for model-generated audio turns, not the current channel voice stack. Voice transcription and TTS still use their dedicated `[transcription]` and `[tts]` provider configs because they call direct speech APIs with provider-specific request shapes, voices, formats, and language hints.

Do not configure WhatsApp, Telegram, or Matrix voice transcription by setting `audio_generation`. Use `audio_generation` for models that produce audio as an agent output, and keep speech-to-text or text-to-speech provider keys in the voice configs until the voice stack is moved onto a typed lane contract.

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
