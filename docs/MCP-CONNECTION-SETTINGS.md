# Quality, codecs, audio and connection diagnostics

The controller reuses stock session settings and decoding statistics without modifying the controlled client. `rd_connection_settings_get` reads settings and observations; `rd_connection_settings_set` changes one setting through `change`. Specify `ui_session_id` when multiple desktop views exist; it is optional for a single view.

Writes require a ready desktop and AI control and use existing `operation_id` deduplication. `confirmed: true` means the local setting was applied/read back; it **does not confirm matching remote codec, FPS, audio or image quality**. `remote_effect_confirmed` is explicitly false. Read metrics for the actual negotiated result.

## Settings

| change.setting | Parameters and limits |
| --- | --- |
| `quality` | `preset: best/balanced/low/custom`; custom requires `quality: 10..2000` and accepts optional `fps: 5..120`; other presets do not accept quality/fps |
| `codec` | `preference: auto/vp8/vp9/av1/h264/h265`; accepts only entries in available_codecs, the intersection of stock controller decoding and peer encoding support |
| `true_color` | `enabled`; enabling requires peer >=1.2.4 and a currently observed VP9 or AV1 codec |
| `audio_muted` | `enabled`; requires remote audio permission; muting does not revoke that permission |
| `quality_overlay` | `enabled`; saves the peer preference and updates the target view's local quality overlay |

Non-direct sessions through public servers disallow custom FPS and cap quality at 100. Peers <1.2.0 disallow custom FPS; peers <1.2.2 cap quality at 100. The interface follows toolbar limits, rejecting out-of-range values instead of silently downgrading them. Custom quality without fps sends no additional FPS change.

Settings are persistent peer preferences. Preset changes retain stock FPS handling. `settings.custom_fps` is the saved custom preference, separate from actual FPS. Codec preference and actual codec are also returned separately. The support list does not guarantee subsequent hardware encoder/decoder initialization; actual observations determine the result.

## Metrics and diagnostics

- speed, FPS, delay, target bitrate, codec and chroma each include `value/known/observed_at/age_ms/fresh/unit`.
- speed retains the stock format with units. FPS retains the current view's stock representation, which may contain multiple values when all displays are shown. delay is in milliseconds; target bitrate retains the stock field value and unit annotation.
- Missing values are unknown. Speed, frame rate, delay and target bitrate become stale after 10 seconds. Codec/chroma are negotiated state retained until an update or reconnect. Metrics are not fresh after disconnect; reconnection clears observations from the previous connection.
- connection reports readiness, encryption, direct/relay status, transport type, public-server use, peer platform/version, stock permission overrides and view-only mode. The outer session uses the existing MCP session-state and control structure.
- `permission_overrides` contains permission messages actually received by the stock client. At desktop login the peer reports only denied audio permission; audio is available on an authenticated desktop if no denial was reported. Query results do not grant permission.
- The quality overlay is local UI; mute and codec options take effect through the stock protocol. Actual FPS may be below the target for a static desktop; this is an expected observation.

## Validation record

- Stock Windows RustDesk 1.4.9 over a self-hosted relay: best/balanced/low and custom quality 65 with target FPS 24 were applied and read back successfully. Original settings were restored afterward.
- VP8, VP9, AV1, H264 and H265 were selected individually, with actual negotiated codecs and subsequent new screenshots verified. VP9 true color was observed as 4:4:4.
- Quality below the minimum, excessive FPS, presets mixed with custom parameters and unknown codec values were rejected. Mute/overlay switches read back correctly, and the local GUI displayed the quality overlay.
- After audio permission was revoked locally on the controlled machine, the settings interface returned PERMISSION_DENIED without changing the mute preference. Restoring permission made mute available again; original settings were restored.
- All six metrics returned timestamps and freshness. Reconnection retained peer preferences, and all known metric timestamps belonged to the new connection.
- This environment supported every listed codec. Valid codecs unsupported by either side, public-relay limits and older-version limits were not tested in matching live environments; stock capability checks and restrictions remain in effect.
- After disconnect, connection.ready=false, codecs_known=false and all metrics fresh=false; setting writes were rejected. Queries recovered after reconnecting and authenticating.
- Automated tests: all 55 automation and 11 MCP tests passed. New Flutter files had no analysis issues; related existing files retained only pre-existing notices. Basic sessions, control, screenshots, operation deduplication, disconnect/reconnect and terminal regression checks passed.
- Strict signature verification passed for both the installed application and the application extracted from the release ZIP.
