# HTTP API

[English](api.md) | 中文 · [← 返回首页](../README.zh-CN.md)

服务端提供一套小型 REST 接口，界面用到的功能都能通过它完成；另外还提供
OpenAI 兼容接口，现成的 TTS 客户端无需改动即可接入。Base URL 为
`http://127.0.0.1:8787`（修改地址与端口见 [配置](configuration.zh-CN.md)）。

来自回环地址的请求受信任，不需要任何凭证。[手机遥控](remote.zh-CN.md) 使用的
局域网监听则需要设备令牌（`Authorization: Bearer …` 或 `laverock_device`
Cookie）；以下均指回环监听。

## 接口

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/health` | 服务状态 |
| GET | `/api/engines` | 已注册引擎及其默认参数 |
| POST | `/api/engines/:id/synthesize` | multipart：`text` 加 `profile_id` 或（`prompt` WAV + `prompt_text`），可附引擎参数 → WAV |
| GET | `/api/profiles` | 已保存声音及时长、大小、素材段数 |
| POST | `/api/profiles` | multipart：`name`、`prompt_text`、`prompt`（WAV），或重复的 `capture` id → 保存声音 |
| POST | `/api/profiles/:id` | multipart：`name`、`prompt_text`、可选 `prompt` → 编辑声音 |
| POST | `/api/profiles/:id/captures` | JSON `{captures: [id, …]}` → 替换该声音引用的素材 |
| GET | `/api/profiles/:id/audio` | 该声音交给引擎的参考音频（`?engine=` 决定按哪个引擎的上限拼接） |
| DELETE | `/api/profiles/:id` | 删除已保存声音（其素材留在素材库） |
| GET | `/api/profiles/:id/export` | 导出便携 `.lvvoice.zip`（清单 + WAV） |
| POST | `/api/profiles/export` | JSON `{ids: [...]}` → 打包为单个 `.lvvoices.zip` |
| POST | `/api/profiles/import` | multipart `file`：单个或批量 `.lvvoice(s).zip`；`strategy=skip`（默认）/`copy`/`overwrite` |
| GET | `/api/captures` | 素材库：转写、检查项与重复标记 |
| POST | `/api/captures` | multipart：`audio`（WAV）、可选 `name`、`prompt_text` 与 `enhance`（`denoise` / `vocals` / `both`）→ 新素材 |
| POST | `/api/captures/:id` | multipart：`name` / `prompt_text` → 编辑素材 |
| GET | `/api/captures/:id/audio` | 素材音频（`?original=1` 返回增强前的原始上传） |
| POST | `/api/captures/:id/enhance` | JSON `{mode}` → 从原始录音重新处理，`off` 恢复原声 |
| GET | `/api/enhance` | 已安装的增强阶段 → `{denoise, vocals}` |
| POST | `/api/captures/:id/transcribe` | 对素材重新做语音识别 |
| DELETE | `/api/captures/:id` | 删除素材（引用它的声音同步去掉这一段） |
| POST | `/api/captures/dedupe` | 删除内容完全相同的素材并重指向声音 → `{removed}` |
| GET | `/api/history` | 已生成音频，最新在前 |
| GET | `/api/history/:id/audio` | 生成的音频（`?format=mp3` 导出 MP3） |
| DELETE | `/api/history/:id` · `/api/history` | 删除单条 / 全部生成记录 |
| POST | `/api/audio/mp3` | 请求体：WAV → MP3（内置纯 Rust 编码器，无需 ffmpeg） |
| POST | `/api/transcribe` | multipart：`audio`（WAV）→ 转写文字（需安装 SenseVoice 模型） |
| GET | `/api/session` | 回环/配对状态（公开） |
| POST | `/api/pair/start` | 仅桌面：生成五分钟配对码 + 二维码 |
| POST | `/api/pair/claim` | 用配对码换取设备 Cookie（公开） |
| GET | `/api/devices` · DELETE `/api/devices/:id` | 仅桌面：列出 / 撤销手机 |
| GET/POST | `/api/settings/lan` | 仅桌面：开关局域网监听 |
| GET/POST | `/api/settings` | 仅桌面：下载代理、镜像、机器默认界面语言 |
| GET | `/api/models` | 仅桌面：目录、安装状态、任务日志尾部（`?lang=en` 返回英文介绍） |
| POST | `/api/models/:id/install` | 仅桌面：后台开始下载 |
| POST | `/api/models/reload` | 仅桌面：重新扫描 `models/*/engine.json` |

一条声音可以由素材库中的一段或多段**素材**组成：`POST /api/profiles` 重复传
`capture` 字段即可；`GET /api/engines` 返回的 `reference_limit_secs` 是拼接时的
长度上限，因为每个引擎只在那段长度内取条件。

引擎参数（`speed`、`num_steps`、`cfg`、`seed` 等）由各引擎的清单声明，服务端按
同一份列表校验，界面也用同一份列表生成控件；`GET /api/engines` 可查看。引擎 id
就是[模型目录](models.zh-CN.md#目录)中的目录 id。

## 示例

```sh
curl -X POST http://127.0.0.1:8787/api/engines/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/synthesize \
  -F text='你好，这是一段测试。' \
  -F prompt=@reference.wav \
  -F prompt_text='参考音频里说的话' \
  -o out.wav
```

使用已保存的声音，不必每次上传参考音频：

```sh
curl http://127.0.0.1:8787/api/profiles                       # 先找到 id
curl -X POST http://127.0.0.1:8787/api/engines/voxcpm2/synthesize \
  -F text='用保存的音色说一句话。' \
  -F profile_id=p1793075127-c66c \
  -o out.wav
```

## OpenAI 兼容 API

针对 OpenAI TTS API 编写的客户端——官方 SDK、Open WebUI、Home Assistant、
SillyTavern、脚本等——只需把 base URL 改成本机地址。无需 API key，
`Authorization` 头会被接受但忽略。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/v1/models` | 已安装引擎及 `tts-1` 别名 |
| GET | `/v1/models/{id}` | 单个模型条目 |
| POST | `/v1/audio/speech` | JSON 请求体 → 音频 |

`model` 填已安装引擎 id，或 `tts-1` / `tts-1-hd` / `gpt-4o-mini-tts`（使用所请求
声音对应的引擎）。`voice` 对预置音色引擎填音色 id（`af_heart`、`zm_xiaoni` 等），
对克隆引擎填已保存声音的名称或 id。`response_format` 支持 `wav`（默认）、`mp3`、
`pcm`（无头 16 位小端单声道）。错误返回 OpenAI 的
`{"error": {"message", "type", "param", "code"}}` 结构。

```sh
curl http://127.0.0.1:8787/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{"model":"tts-1","input":"你好，世界。","voice":"zm_xiaoni","response_format":"mp3"}' \
  -o speech.mp3
```

在官方 Python SDK 中：

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8787/v1", api_key="unused")
resp = client.audio.speech.create(
    model="tts-1", voice="zm_xiaoni", input="你好，世界。"
)
resp.write_to_file("speech.mp3")
```
