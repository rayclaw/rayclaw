---
name: nano-banana
description: "Use this skill for AI image generation and editing powered by Google Gemini. Triggers include: any request to generate, create, draw, or make an image/picture/illustration/artwork from a text description; any request to edit, modify, transform, or alter an existing image; requests for style transfer or combining multiple reference images; requests to continue editing a previously generated image. Also use when the user mentions 'nano-banana', 'Gemini image', or asks for visual content creation. Do NOT use for screenshots, PDF rendering, chart/graph creation, or non-AI image tasks."
compatibility:
  os:
    - linux
  deps:
    - node
---

# Nano Banana — AI Image Generation & Editing

Generate and edit images using Google Gemini 2.5 Flash via the `nano-banana` MCP server.

## Available MCP Tools

All tools are prefixed with `mcp_nano-banana_` when called. There are 6 tools:

### 1. `mcp_nano-banana_generate_image` — Generate from text

Create a new image from a text prompt.

**Parameters:**
| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `prompt` | string | ✅ | Text describing the image to generate |

**Example call:**
```json
{"prompt": "A serene Japanese garden with cherry blossoms at sunset, watercolor style"}
```

**Behavior:** Generates an image and saves it to `./generated_imgs/generated-[timestamp]-[id].png`. Returns the file path and inline image content.

### 2. `mcp_nano-banana_edit_image` — Edit an existing image

Apply modifications to a specific image file on disk.

**Parameters:**
| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `imagePath` | string | ✅ | Full file path to the image to edit |
| `prompt` | string | ✅ | Description of modifications to make |
| `referenceImages` | string[] | ❌ | Optional array of file paths to reference images for style/content transfer |

**Example call:**
```json
{
  "imagePath": "./generated_imgs/generated-1234567890-abc.png",
  "prompt": "Make the sky more dramatic with orange and purple tones",
  "referenceImages": []
}
```

### 3. `mcp_nano-banana_continue_editing` — Continue editing last image

Continue editing the most recently generated/edited image in the session (no file path needed).

**Parameters:**
| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `prompt` | string | ✅ | Modifications/changes to make |
| `referenceImages` | string[] | ❌ | Optional reference images for style transfer |

**Example call:**
```json
{"prompt": "Add a rainbow in the background"}
```

### 4. `mcp_nano-banana_get_last_image_info` — Get last image info

Returns metadata about the last generated/edited image (path, file size, last modified).

**Parameters:** None.

### 5. `mcp_nano-banana_configure_gemini_token` — Set API key

Persist a Gemini API key to local config.

**Parameters:**
| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `apiKey` | string | ✅ | Gemini API key |

### 6. `mcp_nano-banana_get_configuration_status` — Check config

Check whether the API key is configured and its source.

**Parameters:** None.

## Workflow

### Basic image generation

1. User asks for an image → call `mcp_nano-banana_generate_image` with a detailed prompt
2. The tool returns inline image content + saved file path
3. Send the saved file to the user via `send_message` with `attachment_path`

### Iterative editing

1. Generate initial image with `generate_image`
2. User requests changes → use `continue_editing` (uses last image automatically)
3. Repeat as needed — each edit builds on the previous result

### Edit existing image

1. User provides an image path → call `edit_image` with the path and modification prompt
2. For style transfer: include reference images in `referenceImages` array

### Sending results to user

After any generation/edit, the image is saved to disk. **Always send the file to the user:**

```
send_message(attachment_path="/full/path/to/generated_imgs/filename.png", caption="描述")
```

## Usage Guidance

- **Prompt quality matters**: Be specific about style, composition, colors, lighting. Translate vague user requests into detailed prompts.
- **The model is Gemini 2.5 Flash Image**: Good at photorealistic images, illustrations, and style transfer. Less suited for precise text rendering in images.
- **File location**: All generated images land in `./generated_imgs/` relative to the rayclaw working directory (typically `/home/ubuntu/rayclaw/generated_imgs/`).
- **Session state**: The MCP server tracks the last generated image per session, so `continue_editing` works across a conversation.
- **API key**: Configured via `GEMINI_API_KEY` env var in `mcp.json`. If the user reports auth errors, check with `get_configuration_status`.
- **Rate limits**: Gemini API has rate limits. If you get rate limit errors, wait a moment and retry.
- **Image format**: Output is always PNG.
