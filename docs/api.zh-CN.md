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
| GET | `/api/health` | 服务状态，含是否找到 `ffmpeg` |
| GET | `/api/engines` | 已注册引擎及其默认参数 |
| POST | `/api/engines/:id/synthesize` | multipart：`text` 加 `profile_id` 或（`prompt` WAV + `prompt_text`），可附引擎参数 → WAV；加 `x-laverock-job: 1` 则作为后台任务运行，或加 `x-laverock-stream: 1` + `stream_audio=1` 逐句流式下发 |
| POST | `/api/engines/:id/unload` | 释放该模型占用的内存（仍保持安装）→ `204` |
| POST | `/api/engines/unload` | 释放所有已加载模型 → `204` |
| GET | `/api/system` | 服务进程内存、常驻子进程与已加载引擎的推理后端 |
| GET | `/api/jobs/:id` | 任务状态、进度与产物 |
| GET | `/api/jobs/:id/events` | 任务的 Server-Sent Events（先发快照，再回放并跟进实时事件） |
| POST | `/api/jobs/:id/cancel` | 请求停止任务 → `202` |
| GET | `/api/jobs/:id/files/:name` | 下载任务产物 |
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
| GET | `/api/captures/:id/subtitles?format=srt\|vtt` | 将该素材已存的分段导出为字幕文件 |
| DELETE | `/api/captures/:id` | 删除素材（引用它的声音同步去掉这一段） |
| POST | `/api/captures/dedupe` | 删除内容完全相同的素材并重指向声音 → `{removed}` |
| GET | `/api/history` | 已生成音频，最新在前 |
| GET | `/api/history/:id/audio` | 生成的音频（`?format=mp3` 导出 MP3） |
| DELETE | `/api/history/:id` · `/api/history` | 删除单条 / 全部生成记录 |
| POST | `/api/audio/mp3` | 请求体：WAV → MP3（内置纯 Rust 编码器，无需 ffmpeg） |
| GET | `/api/tools` | 本机可用的音频工具 → `{denoise, separation, mp3}` |
| POST | `/api/tools/enhance` | multipart：`audio`（WAV）与 `mode`（`denoise` / `vocals` / `instrumental` / `both`）→ `202` 后台任务；单轨为 WAV，`both` 另附 `stems.zip` |
| GET | `/api/projects` | 字幕项目列表，最近更新的在前 |
| POST | `/api/projects` | multipart：`audio`（WAV）或 `media`/`video`（ffmpeg 能读取的任意格式），可选 `name` 与 `subtitles`（SRT/VTT 文本）→ 项目；不带字幕时分段为空，等待转写 |
| GET | `/api/projects/:id` | 单个项目 |
| PATCH | `/api/projects/:id` | JSON `{name?, source_language?, target_language?, segments?}` → 项目 |
| DELETE | `/api/projects/:id` | 删除项目及其源音频 |
| GET | `/api/projects/:id/source` | 项目的音轨（WAV） |
| GET | `/api/projects/:id/media` | 上传的原始媒体文件（视频或压缩音频） |
| POST | `/api/projects/:id/transcribe` | 对源音频重新语音识别并替换全部分段 → 项目 |
| POST | `/api/projects/:id/synthesize` | 配音后台任务：multipart，可重复 `segment` 只重跑指定分段 → `202`；为每段写出 `segments/<id>.wav` |
| GET | `/api/projects/:id/segments/:segment/audio` | 某一段已生成的配音 |
| POST | `/api/projects/:id/mix` | multipart：可选 `background_volume`、`voice_volume`（0–4）与 `ducking` → `202`；写出并记录 `mix.wav` |
| POST | `/api/projects/:id/export?format=video\|audio` | `202` 任务；`video` 把选定音轨与可选软字幕封装回源视频，`audio` 重新混音 |
| GET | `/api/projects/:id/export?format=srt\|vtt` | 项目的分段 → 可下载的 SRT 或 WebVTT |
| GET | `/api/projects/:id/files/:name` | 已记录的导出文件（`mix.wav`、`export.mp4`） |
| GET | `/api/projects/:id/subtitles?format=srt\|vtt` | 项目的分段 → 可下载的 SRT 或 WebVTT |
| POST | `/api/transcribe` | multipart：`audio`（WAV）→ 转写文字（需安装 SenseVoice 模型） |
| POST | `/api/transcribe/details` | 同样的输入 → `{text, segments: [{start, end, text}], language, duration}` |
| POST | `/api/transcribe/subtitles?format=srt\|vtt` | 同样的输入 → 可下载的 SRT 或 WebVTT（`format=vtt`；默认 `srt`） |
| GET | `/api/session` | 回环/配对状态（公开） |
| POST | `/api/pair/start` | 仅桌面：生成五分钟配对码 + 二维码 |
| POST | `/api/pair/claim` | 用配对码换取设备 Cookie（公开） |
| GET | `/api/devices` · DELETE `/api/devices/:id` | 仅桌面：列出 / 撤销手机 |
| GET/POST | `/api/settings/lan` | 仅桌面：开关局域网监听 |
| GET/POST | `/api/settings` | 仅桌面：下载代理、镜像、机器默认界面语言、ffmpeg 路径 |
| GET | `/api/models` | 仅桌面：目录、安装状态、任务日志尾部（`?lang=en` 返回英文介绍） |
| POST | `/api/models/:id/install` | 仅桌面：后台开始下载 |
| POST | `/api/models/reload` | 仅桌面：重新扫描 `models/*/engine.json` |

一条声音可以由素材库中的一段或多段**素材**组成：`POST /api/profiles` 重复传
`capture` 字段即可；`GET /api/engines` 返回的 `reference_limit_secs` 是拼接时的
长度上限，因为每个引擎只在那段长度内取条件。

带时间戳的转写来自 SenseVoice：它的 CTC 解码器会为每个 token 给出时间，服务端
再聚合成句子级的分段。不提供 token 时间戳的模型则会按停顿切分音频兜底。
`POST /api/captures/:id/transcribe` 会把文字和分段一起存入素材；手动修改转写
文字会清空分段，因为它们已不再对应。分段时间是相对音频开头的秒数。

`POST /api/tools/enhance` 不直接返回音频，而是启动后台任务：人声分离大约需要
两倍实时时间。任务在分离或降噪期间上报 `stage` 事件，结果写成任务文件：
`denoised.wav`、`vocals.wav`、`instrumental.wav`；`both` 则同时给出
`vocals.wav`、`instrumental.wav` 与 `stems.zip`。上传仅支持 WAV，最大 200 MB、
30 分钟；取消会终止分离进程并删除临时目录。MP3 导出复用 `POST /api/audio/mp3`。

**项目**把一段源媒体和一组带时间的字幕绑定在一起。`POST /api/projects` 可以
导入现成的 SRT/VTT 字幕；不导入时项目从空分段开始，由
`POST /api/projects/:id/transcribe` 用 SenseVoice 模型补齐。`PATCH` 会整体替换
分段列表，因此编辑器每次提交所有行；时间必须有限、非负、`end > start`，且起点
落在音频长度内（允许重叠）。每个分段还带有 `translated`、`tts_path` 和 `status`
字段。

源可以是 WAV，也可以是视频或压缩音频（MP4、MOV、MKV、MP3、M4A 等）——后者需要
本机有 `ffmpeg`/`ffprobe`。非 WAV 上传会流式写入磁盘而不是读进内存，用 `ffprobe`
读取时长与画面信息，再把第一条音轨提取为单声道 48 kHz WAV。`source.kind` 为
`video` 或 `audio`；视频项目还会记录 `width`、`height` 和 `fps`。
`/api/projects/:id/source` 始终提供提取出的 WAV 音轨（转写、配音与混音都读它），
`/media` 则提供原始上传文件供编辑器播放。上传上限为 1 GB（WAV 为 200 MB，因为
它会被解码进内存）。`GET /api/health` 会报告是否找到 `ffmpeg`；设置里可以显式填写
`ffmpeg_path`，否则依次查找 `LAVEROCK_FFMPEG` 与 `PATH`。

配音使用项目保存的 `engine_id` 与 `voice_id`（通过 `PATCH` 设置；`voice_id` 为空
表示用引擎自带默认音色，适合 Kokoro 这类预设引擎）。`POST /api/projects/:id/synthesize`
作为后台任务运行：`translated` 非空时朗读译文，否则朗读原文，并为每段写一个
WAV。重复传 `segment` 字段可只重跑指定分段。`tts_path`、`tts_duration` 与
`status` 由服务端维护，客户端不能设置；修改某段原文或译文会删除过期的配音并把
该段重置为 `pending`。

`POST /api/projects/:id/mix` 把已生成的配音按起始时间铺到源音频上：源音频作为
背景，按 `background_volume` 缩放；开启 `ducking` 时，人声期间背景会降到 35%，
两端各有 100 ms 过渡。输出为源采样率的单声道，长度至少覆盖最后一段配音（即使
超出源音频），因此较长的配音不会被截断。文件保存为项目导出 `mix.wav`（同时也是
任务文件），由 files 接口提供下载；`PATCH` 也接受同样的 `mix` 对象。

`POST /api/projects/:id/export?format=video` 用于封装视频项目：画面直接复制
（`-c:v copy`），音轨取已记录的混音（multipart `audio=mix`，默认；必须已混音）
或源视频自带音轨（`audio=original`），字幕以内嵌软字幕加入（`subtitles=soft`，
默认）或省略（`subtitles=none`）。软字幕在 MP4/MOV 中使用 `mov_text`，Matroska
使用 SRT，WebM 使用 WebVTT；容器不支持时会报错，除非关闭字幕。输出按项目时长
截断，保证音画同时结束，保存为 `exports/export.<容器扩展名>`，以
`kind: "video"` 记录，并由 files 接口按正确的媒体类型提供。任务会上报 `stage`
事件，完成后导出文件即可下载。`format=audio` 重新执行混音；`GET` 加
`format=srt|vtt` 则下载字幕文件。

引擎参数（`speed`、`num_steps`、`cfg`、`seed` 等）由各引擎的清单声明，服务端按
同一份列表校验，界面也用同一份列表生成控件；`GET /api/engines` 可查看。每个引擎
还带一段本地化 `description`（`{"en": …, "zh": …}`，第三方清单可能直接是字符串），
供模型选择器展示。引擎 id 就是[模型目录](models.zh-CN.md#目录)中的目录 id。

`GET /api/engines` 还会报告每个引擎的实时状态：`loaded`、`resident`（是否常驻
子进程）、该进程的 `pid`，以及 `backend`（推理后端：`onnx`、`audiocpp`、
`llama.cpp` 或 `python`）。`GET /api/system` 在此基础上汇总内存占用（RSS），
模型卡片与设置页的资源面板用的就是它。`POST /api/engines/:id/unload` 只释放
内存，不删除磁盘文件；下次合成时会自动重新加载。

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

## 流式音频

`x-laverock-stream: 1` 已经把响应变成 Server-Sent Events（`stage`、`steps`、
`wav`、`error`）。再加 multipart 字段 `stream_audio=1`，服务端会把长文本
逐句合成，每完成一句就下发一段：

```text
event: audio
data: {"i":0,"sr":24000,"ch":1,"wav":"<base64 WAV>"}
```

每个分片都是完整的 WAV 文件，客户端可以在下一句还在生成时就开始解码并
排队播放。引擎没有长文本策略时，服务端按句子边界（约 100 字）自行切分；
引擎自带策略时由引擎自己的分段器流式回调。全部完成后，完整音频仍以最后
一个 `wav` 帧下发，并只在历史里记录一条。流式音频只对单个候选项
（`samples=1`）和语音生效；音乐与多候选项请求会忽略该字段。不传该字段时
响应与原来逐字节一致。

## 后台任务

长时间生成可以比发起它的请求活得更久。在 `POST /api/engines/:id/synthesize`
上带 `x-laverock-job: 1`，服务端会立刻返回 `202 {"job_id": "…"}`，任务转入
后台继续执行。任务保存在服务进程内，产物在 `data/jobs/<id>/` 下保留一天。

`GET /api/jobs/:id/events` 是 SSE 流：第一帧是 `snapshot`（`id`、`kind`、
`status`、`progress`、`message`、`artifacts`），随后回放已缓存的事件，再
跟进实时事件，直到 `done` 或 `error`。事件类型为 `stage`、`progress`、
`audio`、`artifact`、`done`、`error`。断线重连会补发错过的事件，所以刷新
页面可以接着看进度而不是丢掉这次运行。`POST …/cancel` 置取消标志，工作
线程会在两次生成之间停下。

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
