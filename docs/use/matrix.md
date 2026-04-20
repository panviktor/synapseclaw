# Matrix

Matrix support lets Synapseclaw receive and answer messages through a Matrix channel, including self-hosted deployments. This is a channel integration, not a separate agent brain.

Configuration and operations details are still more operator-oriented than user-oriented. Use [operate/config.md](../operate/config.md) and [operate/deploy.md](../operate/deploy.md) when wiring Matrix into the service fleet.

## Voice Messages

Synapseclaw sends generated voice replies as Matrix voice messages, so clients that support `m.voice` should render them as a voice bubble rather than as a plain file attachment. The audio bytes stay in the provider-native format; for example, the current Groq Orpheus path returns WAV, while some other providers can return Ogg/Opus or MP3.

Known behavior: Matrix client playback support is not identical across web, desktop, and mobile clients. Synapseclaw does not do local best-effort WAV-to-Ogg conversion for Matrix voice replies, because invalid or partial Ogg/Opus output is worse than a correctly labeled provider-native bubble; if a target client requires strict Ogg/Opus voice payloads, choose a speech synthesis provider/model that returns valid Ogg/Opus directly.
