# Provider reference and boundaries

Research basis: [image-generation-dossier.md](../../../../../../.cas/artifacts/cas-1c67/research/image-generation-dossier.md),
compiled 2026-08-29. Verify live provider documentation before adding an
integration; model names and pricing can change.

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
work, or `gemini-3-pro-image` (Nano Banana Pro) for finals and dense copy. A
minimal request is:

```bash
curl -sS "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-image:generateContent" \
  -H "x-goog-api-key: $GEMINI_API_KEY" -H "Content-Type: application/json" \
  -d '{"contents":[{"parts":[{"text":"{prompt with style tokens}"}]}]}'
```

The response carries base64 image data in an `inlineData` part. Reference
images are additional `inlineData` parts. Nano Banana does not expose a
first-class transparent-background flag; request isolation positively and
verify alpha after decoding. All generated images carry Google's SynthID
watermark. Imagen is retired as of 2026-08-17 and is not a valid fallback.

## Optional and explicitly unwired add-ons

The following snippets preserve the research escape hatches for a future,
separately authorized integration. They are documentation only: this skill
does not read their keys, call their endpoints, or route around a missing
`GEMINI_API_KEY`.

### Recraft V4 / V4.1 — unwired SVG specialist

Recraft is the dossier's production-grade SVG/vector option and supports saved
styles, vectorization, and background removal. It would use
`RECRAFT_API_TOKEN` and an OpenAI-compatible endpoint, but it is intentionally
unwired under the current no-new-paid-services scope:

```bash
curl -sS https://external.api.recraft.ai/v1/images/generations \
  -H "Authorization: Bearer $RECRAFT_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"model":"recraftv4_1_vector","style":"vector_illustration","prompt":"…"}'
```

### OpenAI GPT Image 2 — unwired transparency/OG option

`gpt-image-2` is a strong raster and transparency option and would use
`OPENAI_API_KEY`. It requires organization verification for image models. It
is not called by this skill:

```bash
curl -sS https://api.openai.com/v1/images/generations \
  -H "Authorization: Bearer $OPENAI_API_KEY" -H "Content-Type: application/json" \
  -d '{"model":"gpt-image-2","prompt":"…","size":"1200x630","background":"transparent","output_format":"png"}'
```

### Ideogram 3.0 — unwired typography option

Ideogram's `DESIGN` style, quoted copy, seed, palette, and style-reference
parameters are useful for typography-heavy logos. It would use
`IDEOGRAM_API_KEY` and multipart form data. It is not called by this skill:

```bash
curl -sS -X POST https://api.ideogram.ai/v1/ideogram-v3/generate \
  -H "Api-Key: $IDEOGRAM_API_KEY" \
  -F 'prompt=A logo with exact text "…"' -F style_type=DESIGN -F aspect_ratio=1x1
```

### Black Forest Labs FLUX.2 — unwired hosted option

FLUX.2 offers strong hex-color control, seeds, and multi-reference editing;
the hosted API would use `BFL_API_KEY`, submit asynchronously, and poll its
`polling_url`. It is not called by this skill:

```bash
curl -sS -X POST https://api.bfl.ai/v1/flux-2-pro \
  -H "x-key: $BFL_API_KEY" -H "Content-Type: application/json" \
  -d '{"prompt":"…","width":1024,"height":1024}'
```

FLUX.2 klein's local open weights are a possible free path when a machine has
roughly 13GB VRAM and the operator accepts local model setup. Hardware was not
assumed or provisioned here, so this is documentation only. Midjourney has no
official public API and Stability has pivoted away from a competitive image
API; neither is a valid integration target.
