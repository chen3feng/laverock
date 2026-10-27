# laverock

[English](README.md) | 中文

本地优先的语音克隆工作室。模型全部在你自己的电脑上运行；同一局域网内的手机之后可以作为遥控器接入。

状态：**MVP**。七个引擎——ZipVoice（中英文，ONNX，约 160 MB）、
Kokoro-82M（54 个预置音色，8 种语言，ONNX，约 135 MB，不支持克隆）、
VoxCPM2（30 种语言，GGUF/llama.cpp，约 3.3 GB）、Qwen3-TTS（10 种语言，
官方 Python 包运行在隔离 venv 中）、LongCat-AudioDiT（波形潜空间扩散，
中英，含 venv 约 7 GB）、dots.tts（2B 连续 AR，48 kHz，24 种语言，约 3 GB GGUF）
与 CosyVoice3（0.5B 流式 LLM TTS，9 种语言 + 粤语，约 2.3 GB GGUF）——统一在
一个本地 HTTP 服务、一个内嵌 Web 界面、一个已保存音色库和局域网手机遥控之后。

预置音色引擎（Kokoro）不需要参考音频：选中后合成页会隐藏已保存音色/录音区，
改为内置音色选择器。界面根据引擎的 `capabilities`（`clone` 与 `preset-voices`）
自动切换。

## 架构

```
laverock-core     引擎 trait + 清单注册表 + ONNX/GGUF 运行时
laverock-server   轻量 HTTP 控制面 + 内嵌单页应用 + 模型安装器
laverock-desktop  Tauri 外壳：托盘、开机自启、服务托管
```

侧栏包含五个部分：

- **合成** —— 选择引擎与一个已保存的声音（或现场录音/上传），输入文字，
  生成后播放或下载为 WAV / MP3。
- **语音素材** —— 已保存的参考声音：试听、新建、编辑（名称、文字稿、替换音频）
  和删除。确认操作使用应用内对话框，因为桌面外壳的 WebKit 没有实现
  `confirm()`。
- **历史** —— 每条生成的 WAV（文字、引擎、时间、时长）：播放、下载、单条删除
  或全部清空；数据目录下最多保留 100 条 / 1 GiB。
- **模型** —— 已安装引擎与可安装模型目录。下载时日志实时滚动到卡片中并带进度条
  （百分比和当前步骤；git/pip/tqdm 等子进程的进度也会解析），完成后引擎注册表
  自动重载。
- **设置** —— 下载代理与镜像、界面语言、手机遥控、版本信息。

可安装模型列在 `crates/laverock-server/assets/catalog.json` 中；服务端执行的
就是命令行里同一批 `scripts/download-*.sh`，并把上面的设置以环境变量传入。

- 新增一个受支持类型的模型，只需在 `models/` 下放一个带 `engine.json` 清单的
  目录——无需改代码。
- 清单可以声明 `parameters` 数组；合成页据此生成控件（名称、`float`/`int`/
  `bool`/`choice` 类型、范围、步长、选项、默认值、提示），服务端按同一份列表校验
  每个提交值。未声明时使用各类型的内置参数：ZipVoice 是语速与扩散步数，
  VoxCPM2 另外有 CFG/温度/种子，非扩散的 Qwen3-TTS 运行器则没有参数。
- 推理使用官方 [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
  Rust crate（静态链接预编译 ONNX Runtime；无需 Python，也不依赖系统库）。

## 环境要求

- Rust（stable，在 1.98 上测试）
- macOS / Linux / Windows；CPU 推理全平台可用。MVP 自带 ZipVoice distill INT8
  （含声码器磁盘占用约 160 MB）。

## 下载

预编译 alpha 包在 [releases 页面](https://github.com/chen3feng/laverock/releases)：

| 平台 | 文件 |
| --- | --- |
| macOS（Apple Silicon） | `Laverock_*_aarch64.dmg` |
| Windows 64 位 | `Laverock_*_x64-setup.exe` |

安装包不含模型权重，需要执行一次下面的下载脚本。macOS 包为 ad-hoc 签名，
Windows 包未签名——首次打开会有一次 Gatekeeper/SmartScreen 提示。

## 快速开始

```sh
# 1. 下载模型（可重复执行）
./scripts/download-model.sh        # ZipVoice，约 160 MB，中英，ONNX
./scripts/download-kokoro.sh       # Kokoro-82M，约 135 MB，8 种语言，预置音色
./scripts/download-stt.sh          # SenseVoice 语音识别，约 234 MB，录音自动转写
./scripts/download-voxcpm2.sh      # VoxCPM2，约 3.3 GB，30 种语言，GGUF
./scripts/download-qwen3tts.sh     # Qwen3-TTS 克隆，隔离 venv + 约 1.5 GB 模型
QWEN3TTS_VARIANT=custom ./scripts/download-qwen3tts.sh  # 9 个预置音色
QWEN3TTS_VARIANT=design ./scripts/download-qwen3tts.sh  # 1.7B 声音设计
./scripts/download-longcat.sh      # LongCat-AudioDiT 1B，隔离 venv + 5.7 GB 权重
./scripts/download-dots.sh         # dots.tts SOAR，48 kHz，audio.cpp 运行时 + 3 GB GGUF
./scripts/download-cosyvoice.sh    # CosyVoice3 0.5B，2.3 GB GGUF（共用运行时）

# 2. 运行
cargo run -p laverock-server
# 打开 http://127.0.0.1:8787
```

同样的下载也可以在界面的 **模型** 页完成；脚本适合无界面安装与 CI。

引擎通过 `models/*/engine.json` 发现。ZipVoice 与 LongCat-AudioDiT 需要参考音频
对应文字；VoxCPM2、Qwen3-TTS 与 dots.tts 仅凭声音即可克隆；CosyVoice3 只有零样本
模板需要文字（跨语种模板不需要）。Kokoro 完全不使用参考音频——它内置 54 个音色，
通过清单声明的 `voice` 参数选择，界面会为它隐藏克隆区。Qwen3-TTS 还可以通过
两种非克隆安装（共用同一 venv）：`QWEN3TTS_VARIANT=custom` 提供九个内置说话人
（Vivian、Ryan、Sohee 等，合成页选择，无需参考音频）；`QWEN3TTS_VARIANT=design`
为 1.7B 模型，通过 instruct 字段的自然语言描述来创造音色。新增一个受支持类型的
模型只需再放一个带清单的目录；需要 Python 的引擎在 `.tools/` 下各自独立的 venv 中
运行，绝不进入核心进程。

Qwen3-TTS 自动选择设备：CUDA 用 bfloat16，Apple MPS 用 float32（codec 上
float16 会产生 NaN），其余走 CPU。以一句短句为例，0.6B 克隆在 Apple Silicon 上
约 25 秒、CPU 约 90 秒（含模型加载）。

LongCat-AudioDiT 是直接工作在波形潜空间的 flow-matching 扩散模型；每次请求可调
NFE 步数、引导强度以及 APG / CFG（见清单中的 `parameters`）。CUDA 上 fp16、
其它平台 fp32；权重和 UMT5 分词器都放在模型目录内，1B 版本约需 6 GB 内存。
单次请求连同参考音频最多生成约 30 秒，因此优先使用 3–10 秒的提示。克隆时会根据
参考时长对齐文字稿：如果音频明显长于文字稿，运行器会在一个静音点自动裁剪
（并在进度行提示），否则目标文本开头会被切掉、第一句消失。请保持文字稿准确。
合成前数字串会转成中文读法（如上游文档所述，该模型对阿拉伯数字处理较差）；
超出单次预算的长文本会自动分段并以短停顿拼接，而不是硬压——此前长文本开头被吞掉
正是这个原因。用
`LONGCAT_MODEL=meituan-longcat/LongCat-AudioDiT-3.5B ./scripts/download-longcat.sh`
可安装更大的 3.5B 版本。

dots.tts（Studio Dots）是 2B 连续 AR 模型，flow-matching 头部驱动 48 kHz
AudioVAE。Laverock 通过 audio.cpp 原生运行时执行它的 GGUF 导出——macOS 用
Metal，其它平台用 CPU（或自行编译的 CUDA/Vulkan 版本）——因此没有 Python
环境，整体安装约 3 GB。克隆默认使用 x-vector 模式（仅凭参考音频），保存的文字稿
不精确也稳定；文字稿准确时「声纹 + 文字」相似度更高。长文本由运行时分段。
Q8_0 检查点在 M4 Pro 实测：RTF 1.3–1.9，48 kHz 输出，峰值内存约 6 GB。
运行时版本在 `scripts/download-dots.sh` 中固定（`DOTS_VERSION`）；
`DOTS_MODEL=dots.tts-mf` 选择 4 步 MeanFlow 导出。

CosyVoice3（FunAudioLLM）是 0.5B 流式 LLM TTS，与 dots.tts 共用 audio.cpp
运行时，两个都装只下载一次运行时。它提供三种模板：零样本克隆（参考音频 + 准确
文字稿，相似度最高）、跨语种克隆（仅凭声音，文字稿不可靠时使用）和指令控制
（用文字描述语气，例如「用温和的语气，语速稍慢」）。Top-K 与流解码步数可逐次
调整，长文本由运行时分段。

安装可选的 SenseVoice 语音识别模型（`download-stt.sh`）后，录音或上传的参考音频会
自动转写并填入文字稿，也可用「转写」按钮手动重跑；支持中英日韩粤，ONNX 本地推理。

录制或选择 3–10 秒干净的参考人声，填入与录音完全一致的文字稿（ZipVoice 靠它对齐
音色和韵律），输入要合成的文字，点击生成即可。

### 界面语言

侧栏左下角可切换**界面语言**：跟随系统（按浏览器语言自动选择）、中文、English。
选择会保存在本浏览器；在电脑端显式选择后还会作为机器默认值写入设置，供新配对的
设备继承。引擎清单里的参数名称与选项、模型目录的介绍文字都会随语言切换；后端返回
的少量校验错误也会在英文界面下翻译。

### 代理与镜像

**设置 → 下载网络**保存在 `<data>/settings.json` 中，并传递给每个模型下载脚本；
绝不影响推理。

| 设置 | 环境变量 | 默认值 |
|---|---|---|
| 代理 | `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`（含小写形式） | 系统浏览器代理 |
| Hugging Face 镜像 | `HF_ENDPOINT` | `https://huggingface.co` |
| PyPI 镜像 | `PIP_INDEX_URL` | PyPI |

下载默认跟随浏览器使用的代理：优先使用已导出的 `HTTPS_PROXY`（或 `ALL_PROXY` /
`HTTP_PROXY`），否则读取 macOS 系统代理（`scutil --proxy`）或 Windows Internet
Settings。选择「手动指定」可覆盖，选择「不使用代理」强制直连；手动地址支持
`http`、`https`、`socks5` 与 `socks5h`。不会解析 PAC 脚本——这种情况下请手动
填写代理。脚本运行时会在继承的排除列表外合并 `no_proxy=127.0.0.1,localhost`，
因此本地镜像仍可使用。

### 桌面外壳

```sh
./scripts/run-desktop.sh
```

在 macOS 上，脚本会把二进制包进一个最小 `.app` 并做 ad-hoc 签名，因为麦克风访问
（`getUserMedia`）需要带 `NSMicrophoneUsageDescription` 的 bundle 和独立的 TCC
身份。从终端直接启动裸二进制会被归到终端名下，通常无声地被拒绝。首次点击录音时
macOS 会弹窗请求权限，之后会被记住。

外壳会替你托管服务端：如果 `127.0.0.1:8787` 没有响应，它会自行启动
`laverock-server`，然后打开同一套 Web 界面窗口。关窗只是隐藏；托盘菜单提供显示/
隐藏、开机自启、打开数据目录和退出。

开发时外壳与独立服务端共用仓库的 `data/` 目录；发布版使用系统应用数据目录。设置
`LAVEROCK_SERVER_BIN` 可显式指定服务端二进制，设置 `LAVEROCK_URL` 可接入其它机器
上的服务端。

### 手机遥控

在桌面界面打开 **手机遥控 → 局域网访问**。服务端会在本机局域网地址上开启第二个
监听（回环监听保持独立），点击 **生成配对二维码** 会得到二维码和六位配对码。

- 手机加载的是与桌面完全相同的单页应用，没有第二套前端。
- **回环可信，局域网监听不可信。** 除 `/`、`/api/session` 和
  `/api/pair/claim` 外，所有请求都需要设备令牌，可通过
  `Authorization: Bearer …` 或 `laverock_device` Cookie（`<audio>` 与流式响应
  需要）提供。
- 配对码一次性使用，五分钟过期，八次错误尝试后失效。只有回环端能生成配对码。
- 令牌为 32 个随机字节，仅以 SHA-256 哈希形式保存在 `data/remote.json`
  （权限 0600）。撤销手机在它下一次请求时生效。
- 配对状态在重启后保留（`lan_enabled` 持久化）。

局域网监听使用自签证书提供 **HTTPS**（位于 `data/tls/`，局域网地址变化时重新
生成），这样手机端才是安全上下文——麦克风和 PWA 安装都需要它。首次访问会提示
证书不受信任，选择「高级 → 继续前往」。二维码旁显示证书 SHA-256 指纹，谨慎的
用户可以核对是否一致。局域网地址在启动时解析——如果机器切换了网络，请关闭再
重新打开局域网访问。

### API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 服务状态 |
| GET | `/api/engines` | 已注册引擎及默认参数 |
| POST | `/api/engines/:id/synthesize` | multipart：`text` 加 `profile_id` 或（`prompt` WAV + `prompt_text`），可选 `speed`、`num_steps` → WAV |
| GET | `/api/profiles` | 已保存声音及时长/大小 |
| POST | `/api/profiles` | multipart：`name`、`prompt_text`、`prompt`（WAV）→ 保存声音 |
| GET | `/api/profiles/:id/audio` | 某条已保存声音的参考音频 |
| POST | `/api/audio/mp3` | 请求体：WAV → MP3（需要 ffmpeg 或 lame） |
| POST | `/api/transcribe` | multipart：`audio`（WAV）→ 转写文字（需下载 SenseVoice 模型） |
| POST | `/api/profiles/:id` | multipart：`name`、`prompt_text`、可选 `prompt` → 编辑声音 |
| GET | `/api/profiles/:id/export` | 导出便携 `.lvvoice.zip`（清单 + WAV） |
| POST | `/api/profiles/import` | multipart：`file` 为 `.lvvoice.zip` → 导入声音 |
| DELETE | `/api/profiles/:id` | 删除已保存声音 |
| GET | `/api/history` | 已生成音频，最新在前 |
| GET | `/api/history/:id/audio` | 生成的音频（`?format=mp3` 导出 MP3） |
| DELETE | `/api/history/:id` · `/api/history` | 删除单条/全部生成记录 |
| GET | `/api/session` | 回环/配对状态（公开） |
| POST | `/api/pair/start` | 仅桌面：生成五分钟配对码 + 二维码 |
| POST | `/api/pair/claim` | 用配对码换取设备 Cookie（公开） |
| GET | `/api/devices` · DELETE `/api/devices/:id` | 仅桌面：列出/撤销手机 |
| GET/POST | `/api/settings/lan` | 仅桌面：开关局域网监听 |
| GET/POST | `/api/settings` | 仅桌面：下载代理与镜像、机器默认界面语言 |
| GET | `/api/models` | 仅桌面：目录、安装状态、任务日志尾部（`?lang=en` 返回英文介绍） |
| POST | `/api/models/:id/install` | 仅桌面：后台开始下载 |
| POST | `/api/models/reload` | 仅桌面：重新扫描 `models/*/engine.json` |

示例：

```sh
curl -X POST http://127.0.0.1:8787/api/engines/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia/synthesize \
  -F text='你好，这是一段测试。' \
  -F prompt=@reference.wav \
  -F prompt_text='参考音频里说的话' \
  -o out.wav
```

### OpenAI 兼容 API

针对 OpenAI TTS API 编写的客户端（官方 SDK、Open WebUI、Home Assistant、
SillyTavern、脚本等）只需把 base URL 改成本机地址即可使用。无需 API key——
`Authorization` 头会被接受但忽略。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/v1/models` | 已安装引擎及 `tts-1` 别名 |
| GET | `/v1/models/{id}` | 单个模型条目 |
| POST | `/v1/audio/speech` | JSON 请求体 → 音频 |

`model` 填已安装引擎 id，或 `tts-1` / `tts-1-hd` / `gpt-4o-mini-tts`
（使用所请求声音对应的引擎）。`voice` 对 Kokoro 填预置音色 id（`af_heart`、
`zm_xiaoni` 等），对克隆引擎填已保存声音的名称或 id。`response_format` 支持
`wav`（默认）、`mp3`、`pcm`（无头 16 位小端单声道）。错误返回 OpenAI 的
`{"error": {"message", "type", "param", "code"}}` 结构。

```sh
curl http://127.0.0.1:8787/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{"model":"tts-1","input":"你好，世界。","voice":"zm_xiaoni","response_format":"mp3"}' \
  -o speech.mp3
```

### 配置

| 环境变量 | 默认值 | 用途 |
|---|---|---|
| `LAVEROCK_HOST` | `127.0.0.1` | 受信任回环监听的绑定地址 |
| `LAVEROCK_PORT` | `8787` | 端口 |
| `LAVEROCK_MODELS_DIR` | `models` | 扫描 `*/engine.json` 的目录 |
| `LAVEROCK_DATA_DIR` | `data` | 声音、设置、配对状态、TLS 证书 |
| `LAVEROCK_SCRIPTS_DIR` | 自动检测 | 模型安装器使用的 `scripts/` |
| `LAVEROCK_FFMPEG` / `LAVEROCK_LAME` | 自动检测 | 显式指定 MP3 编码器路径 |
| `LAVEROCK_TOOLS_DIR` | `<repo>/.tools` | 引擎的构建工具与 Python venv |

打包后的桌面版在应用包内附带 `scripts/`（macOS 为
`Contents/Resources/scripts`，Windows/Linux 在可执行文件旁），下载内容、构建
工具和 venv 放在用户数据目录（macOS 为
`~/Library/Application Support/sh.laverock.desktop/models` 与 `…/tools`），
因此已安装的应用不会读取源码目录。Windows 上应用内安装模型需要 Git for
Windows 提供的 `bash`；下载脚本在各平台其余部分完全一致。

## 模型鸣谢

ZipVoice 模型权重为 Apache-2.0，由
[k2-fsa/ZipVoice](https://github.com/k2-fsa/ZipVoice) 发布、
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) 打包。

Kokoro-82M 权重为 Apache-2.0，由 [hexgrad/Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M)
发布；ONNX 导出与打包来自 sherpa-onnx。

LongCat-AudioDiT 代码与权重为 MIT，由
[meituan-longcat/LongCat-AudioDiT](https://github.com/meituan-longcat/LongCat-AudioDiT)
发布；UMT5 分词器来自 `google/umt5-base`。

CosyVoice3 代码与检查点为 Apache-2.0，由
[FunAudioLLM/CosyVoice](https://github.com/QwenAudio/CosyVoice) 发布；
GGUF 导出同样来自 audio.cpp。

dots.tts 代码与检查点为 Apache-2.0，由
[studio-dots-ai/dots.tts](https://github.com/studio-dots-ai/dots.tts) 发布。
GGUF 导出与原生运行时来自
[0xShug0/audio.cpp](https://github.com/0xShug0/audio.cpp)；权重保留其
Apache-2.0 许可，运行时二进制以未修改形式再分发。

## 许可证

MIT
