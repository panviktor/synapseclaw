# Matrix

Matrix support lets Synapseclaw receive and answer messages through a Matrix channel, including self-hosted deployments. This is a channel integration, not a separate agent brain.

Configuration and operations details are still more operator-oriented than user-oriented. Use [operate/config.md](../operate/config.md) and [operate/deploy.md](../operate/deploy.md) when wiring Matrix into the service fleet.

## Realtime Calls

Matrix is currently the most complete live voice path in SynapseClaw. You can discuss a topic in Matrix text chat, switch into a live audio call, and then continue in text with a compact recap of the call folded back into the room context.

The current call path is:

- MatrixRTC or LiveKit media session
- realtime speech provider for turn detection and transcription
- normal agent runtime on bounded text turns
- speech synthesis back into the same Matrix call

Live call behavior is policy-driven, not hardcoded in the Matrix adapter. Model choice, spoken reply budget, excluded tools, locale fallback, and greetings come from `[agent.live_calls]`, while Matrix stays responsible for transport, media bootstrap, and channel-specific side effects.

For setup details, use [../reference/realtime-calls.md](../reference/realtime-calls.md). That page is the source of truth for the current `6.1` Matrix call runtime.

## Voice Messages

Synapseclaw sends generated voice replies as Matrix voice messages, so clients that support `m.voice` should render them as a voice bubble rather than as a plain file attachment. The audio bytes stay in the provider-native format; for example, the current Groq Orpheus path returns WAV, while some other providers can return Ogg/Opus or MP3.

Known behavior: Matrix client playback support is not identical across web, desktop, and mobile clients. Synapseclaw does not do local best-effort WAV-to-Ogg conversion for Matrix voice replies, because invalid or partial Ogg/Opus output is worse than a correctly labeled provider-native bubble; if a target client requires strict Ogg/Opus voice payloads, choose a speech synthesis provider/model that returns valid Ogg/Opus directly.

## Current Scope

What is solid today:

- Matrix text chat
- Matrix voice messages
- Matrix live audio calls

What is still intentionally not the documented default:

- video calls
- channel-specific prompt hacks
- identical playback behavior across every Matrix client
