# Provider reference and boundaries

Research basis: a provider survey compiled 2026-08-29. Verify live provider
documentation before adding an integration; model names, limits, and pricing
change without notice.

## Wired: Google Nano Banana

This skill currently wires only Google AI Studio's Gemini image endpoint. The
key is `GEMINI_API_KEY`; it is read only from the environment by the helper.
There is no image-provider key in CAS's existing config/secrets convention, so
the missing-key message points the operator to Google AI Studio rather than
creating a new config field. The endpoint is:

```text
POST https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent
header: x-goog-api-key: $GEMINI_API_KEY
```

Use `gemini-3.1-flash-image` (Nano Banana 2) for drafts and ordinary raster
work, or `gemini-3-pro-image` (Nano Banana Pro) for finals and dense copy.
`gemini-3.1-flash-lite-image` (Nano Banana 2 Lite) is a cheaper draft option the
helper does not route to yet. The helper writes the key to a mode-600 header file,
never to curl's arguments, and sends:

```json
{"contents":[{"parts":[{"text":"{prompt with style tokens}"}]}],
 "generationConfig":{"responseModalities":["IMAGE"],
   "imageConfig":{"aspectRatio":"16:9","imageSize":"2K"}}}
```

`generationConfig` is present only when `--aspect` or `--size` is passed.
The response carries base64 image data in an `inlineData` part. Reference
images are additional `inlineData` parts. Nano Banana does not expose a
first-class transparent-background flag; request isolation positively and
verify alpha after decoding. All generated images carry Google's SynthID
watermark. Imagen is retired as of 2026-08-17 and is not a valid fallback.

## Explicitly unwired providers

Documentation only: this skill does not read these keys, call these endpoints,
or route around a missing `GEMINI_API_KEY`. Wiring one is a separately
authorized integration.

| Provider | Key it would use | Strength | Why unwired |
|---|---|---|---|
| Recraft V4 / V4.1 | `RECRAFT_API_TOKEN` | production SVG/vector, saved styles, vectorization | no new paid services in scope |
| OpenAI `gpt-image-2` | `OPENAI_API_KEY` | transparency, OG sizes | needs organization verification; no new paid services |
| Ideogram 3.0 | `IDEOGRAM_API_KEY` | exact typography in logos | no new paid services |
| Black Forest Labs FLUX.2 | `BFL_API_KEY` | hex-color control, multi-reference edits | async polling API; no new paid services |
| FLUX.2 klein (local weights) | none | free local path | needs ~13GB VRAM and local setup |

Midjourney has no official public API and Stability has pivoted away from a
competitive image API; neither is a valid integration target.
