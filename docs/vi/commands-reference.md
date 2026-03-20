# Tham khảo lệnh SynapseClaw

Dựa trên CLI hiện tại (`synapseclaw --help`).

Xác minh lần cuối: **2026-02-20**.

## Lệnh cấp cao nhất

| Lệnh | Mục đích |
|---|---|
| `onboard` | Khởi tạo workspace/config nhanh hoặc tương tác |
| `agent` | Chạy chat tương tác hoặc chế độ gửi tin nhắn đơn |
| `gateway` | Khởi động gateway webhook và HTTP WhatsApp |
| `daemon` | Khởi động runtime có giám sát (gateway + channels + heartbeat/scheduler tùy chọn) |
| `service` | Quản lý vòng đời dịch vụ cấp hệ điều hành |
| `doctor` | Chạy chẩn đoán và kiểm tra trạng thái |
| `status` | Hiển thị cấu hình và tóm tắt hệ thống |
| `cron` | Quản lý tác vụ định kỳ |
| `models` | Làm mới danh mục model của provider |
| `providers` | Liệt kê ID provider, bí danh và provider đang dùng |
| `channel` | Quản lý kênh và kiểm tra sức khỏe kênh |
| `integrations` | Kiểm tra chi tiết tích hợp |
| `skills` | Liệt kê/cài đặt/gỡ bỏ skills |
| `migrate` | Nhập dữ liệu từ runtime khác (hiện hỗ trợ OpenClaw) |
| `config` | Xuất schema cấu hình dạng máy đọc được |
| `completions` | Tạo script tự hoàn thành cho shell ra stdout |
| `hardware` | Phát hiện và kiểm tra phần cứng USB |
| `peripheral` | Cấu hình và nạp firmware thiết bị ngoại vi |

## Nhóm lệnh

### `onboard`

- `synapseclaw onboard`
- `synapseclaw onboard --channels-only`
- `synapseclaw onboard --api-key <KEY> --provider <ID> --memory <sqlite|lucid|markdown|none>`
- `synapseclaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none>`

### `agent`

- `synapseclaw agent`
- `synapseclaw agent -m "Hello"`
- `synapseclaw agent --provider <ID> --model <MODEL> --temperature <0.0-2.0>`
- `synapseclaw agent --peripheral <board:path>`

### `gateway` / `daemon`

- `synapseclaw gateway [--host <HOST>] [--port <PORT>]`
- `synapseclaw daemon [--host <HOST>] [--port <PORT>]`

### `service`

- `synapseclaw service install`
- `synapseclaw service start`
- `synapseclaw service stop`
- `synapseclaw service restart`
- `synapseclaw service status`
- `synapseclaw service uninstall`

### `cron`

- `synapseclaw cron list`
- `synapseclaw cron add <expr> [--tz <IANA_TZ>] <command>`
- `synapseclaw cron add-at <rfc3339_timestamp> <command>`
- `synapseclaw cron add-every <every_ms> <command>`
- `synapseclaw cron once <delay> <command>`
- `synapseclaw cron remove <id>`
- `synapseclaw cron pause <id>`
- `synapseclaw cron resume <id>`

### `models`

- `synapseclaw models refresh`
- `synapseclaw models refresh --provider <ID>`
- `synapseclaw models refresh --force`

`models refresh` hiện hỗ trợ làm mới danh mục trực tiếp cho các provider: `openrouter`, `openai`, `anthropic`, `groq`, `mistral`, `deepseek`, `xai`, `together-ai`, `gemini`, `ollama`, `astrai`, `venice`, `fireworks`, `cohere`, `moonshot`, `glm`, `zai`, `qwen` và `nvidia`.

### `channel`

- `synapseclaw channel list`
- `synapseclaw channel start`
- `synapseclaw channel doctor`
- `synapseclaw channel bind-telegram <IDENTITY>`
- `synapseclaw channel add <type> <json>`
- `synapseclaw channel remove <name>`

Lệnh trong chat khi runtime đang chạy (Telegram/Discord):

- `/models`
- `/models <provider>`
- `/model`
- `/model <model-id>`

Channel runtime cũng theo dõi `config.toml` và tự động áp dụng thay đổi cho:
- `default_provider`
- `default_model`
- `default_temperature`
- `api_key` / `api_url` (cho provider mặc định)
- `reliability.*` cài đặt retry của provider

`add/remove` hiện chuyển hướng về thiết lập có hướng dẫn / cấu hình thủ công (chưa hỗ trợ đầy đủ mutator khai báo).

### `integrations`

- `synapseclaw integrations info <name>`

### `skills`

- `synapseclaw skills list`
- `synapseclaw skills install <source>`
- `synapseclaw skills remove <name>`

`<source>` chấp nhận git remote (`https://...`, `http://...`, `ssh://...` và `git@host:owner/repo.git`) hoặc đường dẫn cục bộ.

Skill manifest (`SKILL.toml`) hỗ trợ `prompts` và `[[tools]]`; cả hai được đưa vào system prompt của agent khi chạy, giúp model có thể tuân theo hướng dẫn skill mà không cần đọc thủ công.

### `migrate`

- `synapseclaw migrate openclaw [--source <path>] [--dry-run]`

### `config`

- `synapseclaw config schema`

`config schema` xuất JSON Schema (draft 2020-12) cho toàn bộ hợp đồng `config.toml` ra stdout.

### `completions`

- `synapseclaw completions bash`
- `synapseclaw completions fish`
- `synapseclaw completions zsh`
- `synapseclaw completions powershell`
- `synapseclaw completions elvish`

`completions` chỉ xuất ra stdout để script có thể được source trực tiếp mà không bị lẫn log/cảnh báo.

### `hardware`

- `synapseclaw hardware discover`
- `synapseclaw hardware introspect <path>`
- `synapseclaw hardware info [--chip <chip_name>]`

### `peripheral`

- `synapseclaw peripheral list`
- `synapseclaw peripheral add <board> <path>`
- `synapseclaw peripheral flash [--port <serial_port>]`
- `synapseclaw peripheral setup-uno-q [--host <ip_or_host>]`
- `synapseclaw peripheral flash-nucleo`

## Kiểm tra nhanh

Để xác minh nhanh tài liệu với binary hiện tại:

```bash
synapseclaw --help
synapseclaw <command> --help
```
