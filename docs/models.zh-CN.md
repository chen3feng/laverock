# 模型

[English](models.md) | 中文 · [← 返回首页](../README.zh-CN.md)

Laverock 的安装包不含模型权重：包里是应用本身，模型按需安装。每个模型就是
一个带 `engine.json` 清单的目录，因此安装模型从不需要更新应用。

## 安装

**在应用里** —— 打开 **模型** 页，在卡片上点击安装。下载日志实时滚动到卡片中并
显示进度条，完成后引擎列表自动刷新。失败的安装可以从同一张卡片重试；脚本都是
可重复执行的。

**在命令行** —— 应用执行的就是这些脚本，无界面安装与 CI 可以直接调用：

```sh
./scripts/download-zipvoice.sh     # ZipVoice，约 160 MB，中英，ONNX
./scripts/download-kokoro.sh       # Kokoro-82M，约 135 MB（INT8；含 fp32 约 450 MB），8 种语言，预置音色
./scripts/download-stt.sh          # SenseVoice 语音识别，约 234 MB，自动转写
./scripts/download-voxcpm2.sh      # VoxCPM2，约 3.3 GB，30 种语言，GGUF
./scripts/download-qwen3tts.sh     # Qwen3-TTS 克隆，隔离 venv + 约 1.5 GB 模型
./scripts/download-longcat.sh      # LongCat-AudioDiT 1B，隔离 venv + 5.7 GB 权重
./scripts/download-dots.sh         # dots.tts SOAR，48 kHz，audio.cpp 运行时 + 3 GB GGUF
./scripts/download-cosyvoice.sh    # CosyVoice3 0.5B，2.3 GB GGUF（共用运行时）
./scripts/download-ming3tts.sh     # Ming-3.0-tts 0.5B，venv + 约 3 GB 权重
./scripts/download-chatterbox.sh   # Chatterbox 多语种，venv + 约 5 GB
./scripts/download-indextts2.sh    # IndexTTS-2，3.6 GB Q8_0 GGUF（共用运行时）
./scripts/download-mosstts.sh      # MOSS-TTS-Delay，仅 macOS/Linux，约 10 GB 权重

# 变体
QWEN3TTS_VARIANT=custom ./scripts/download-qwen3tts.sh    # 9 个预置音色
QWEN3TTS_VARIANT=design ./scripts/download-qwen3tts.sh    # 1.7B 声音设计
MING3TTS_VARIANT=voice_design ./scripts/download-ming3tts.sh
MING3TTS_VARIANT=multilingual ./scripts/download-ming3tts.sh
MING3TTS_VARIANT=ip ./scripts/download-ming3tts.sh        # Ming 内置音色
LONGCAT_MODEL=meituan-longcat/LongCat-AudioDiT-3.5B ./scripts/download-longcat.sh
DOTS_MODEL=dots.tts-mf ./scripts/download-dots.sh         # 4 步 MeanFlow 导出
```

脚本会读取 **设置 → 下载网络** 中的代理与镜像设置（`HTTP_PROXY` /
`HTTPS_PROXY` / `ALL_PROXY`、`HF_ENDPOINT`、`PIP_INDEX_URL`），见
[配置](configuration.zh-CN.md)。Windows 上应用内安装需要 Git for Windows
提供的 `bash`。

模型写入 `<models 目录>/<目录名>/`。从源码运行时 models 目录是 `models/`，
打包版则是应用数据目录；Python 环境与构建工具放在它旁边的 tools 目录。

## 目录

`<引擎 id>` 是 API、模型目录和 OpenAI 兼容接口中使用的名称。

| 引擎 id | 模型 | 模式 | 语言 | 下载体积 |
| --- | --- | --- | --- | --- |
| `sherpa-onnx-zipvoice-distill-int8-zh-en-emilia` | ZipVoice Distill INT8 | 克隆 | 中英 | 约 160 MB |
| `kokoro-int8-multi-lang-v1_0` | Kokoro-82M | 54 个预置音色 | 中英日法西意葡印地 | 约 135 MB（INT8）/ 含 fp32 约 450 MB |
| `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17-int8` | SenseVoice Small INT8 | 语音识别 | 中英日韩粤 | 约 234 MB |
| `voxcpm2` | VoxCPM2 | 克隆、声音设计 | 30 种语言 | 约 3.3 GB |
| `longcat` | LongCat-AudioDiT 1B | 克隆 | 中英 | 约 7 GB（含 venv） |
| `dots` | dots.tts SOAR Q8_0 | 克隆 | 24 种语言 | 约 3.1 GB |
| `cosyvoice` | CosyVoice3 0.5B Q8_0 | 克隆、指令控制 | 9 种语言 + 粤语 | 约 2.3 GB |
| `qwen3tts` | Qwen3-TTS 0.6B | 克隆 | 10 种语言 | 约 2–3 GB（含 venv） |
| `qwen3tts-custom` | Qwen3-TTS 0.6B 预置音色 | 9 个预置说话人、指令 | 10 种语言 | 约 2–3 GB |
| `qwen3tts-design` | Qwen3-TTS 1.7B | 声音设计、指令 | 10 种语言 | 约 4–5 GB |
| `ming3tts` | Ming-3.0-tts 0.5B | 克隆 | 中英德西法韩日 | 约 3 GB + venv |
| `ming3tts-design` | Ming-3.0-tts 0.5B | 声音设计、指令 | 同上 | 共用 |
| `ming3tts-multilingual` | Ming-3.0-tts 0.5B | 克隆、多语种 | 同上 | 共用 |
| `ming3tts-ip` | Ming-3.0-tts 0.5B | 预置音色 | 中英 | 共用 |
| `chatterbox` | Chatterbox Multilingual | 克隆 | 23 种语言 | 约 5 GB（含 venv） |
| `indextts2` | IndexTTS-2 Q8_0 | 克隆 | 中、英 | 约 3.6 GB |
| `mosstts` | MOSS-TTS-Delay | 克隆、多语种 | 20 种语言 | 约 10 GB（安装峰值约 16 GB） |
| `enhance` | 人声清理（GTCRN + BS-RoFormer） | 录音清理 | — | 约 165 MB |

四个 Ming 条目共用同一份权重、同一个 venv 和同一份运行时，只有第一个需要完整
下载，其余只增加几 KB 的清单。

`enhance` 不是引擎：它为素材库增加两个可选的清理步骤。降噪（GTCRN，
16 kHz）去掉室内底噪、风扇和车流声；人声分离（BS-RoFormer，44.1 kHz）去掉
背景音乐、只保留人声。降噪默认开启，一段短录音不到一秒即可完成；分离复用
audio.cpp 运行时，每段录音大约耗时 1–2 倍时长，因此需要手动选择。处理在
录音上传后进行，原始录音会保留，随时能在素材卡上换一种方式重新处理或恢复
原声。

## 能力标记

| 标记 | 含义 |
| --- | --- |
| `clone` | 需要参考音频（部分引擎还需要对应文字稿） |
| `preset-voices` | 内置音色；合成页隐藏声音库，改为音色选择器 |
| `voice-design` | 用自然语言描述在指令栏里创造音色 |
| `instruct` | 接受语气/语速描述，例如「用温和的语气，语速稍慢」 |
| `multilingual` | 可以朗读参考音频之外的语言 |
| `stt` | 语音识别，用于转写参考音频；不是 TTS 引擎 |

## 各引擎说明

**ZipVoice** —— 中英文零样本克隆，静态 ONNX 运行时。体积最小、CPU 上最快，
适合作为默认引擎。克隆必须提供参考音频对应的文字，两者都要准确，否则音色会漂移。

**Kokoro** —— 仅 82M 参数，CPU 上实时合成，54 个官方预置音色覆盖 8 种语言。
不支持克隆：在引擎参数里选择内置音色。纯 ONNX，无需 Python 或编译工具链。

模型有 INT8 与 fp32 两种导出。在 fp32 更快的机器上——CUDA 构建，或没有 AVX2
的 x86 CPU（此时 ONNX Runtime 的 INT8 算子会退化为标量实现）——安装脚本会一并
下载 fp32 文件，引擎自动优先使用它；其它机器只下载 135 MB 的 INT8。Apple
Silicon 保持 INT8：NEON 上量化模型表现很好，也不存在可优先的 CUDA 后端。两个
文件可以共存，删掉 `model.onnx` 即回退到 INT8。

**VoxCPM2** —— 30 种语言，支持零样本克隆与声音设计。通过 audio.cpp 原生运行时
推理（Windows 带 NVIDIA 显卡时使用 CUDA），与 dots.tts、CosyVoice3 共用，并常驻
在服务进程里。参考音频超过 14 秒时会从开头自动截断到静音点，建议使用 3–10 秒的
干净人声。M4 Pro 上 RTF 0.62，加载后内存约 6.5 GB。

**LongCat-AudioDiT** —— 直接工作在波形潜空间的 flow-matching 扩散模型，每次请求
可调 NFE 步数与引导强度（APG / CFG）。CUDA 上 fp16、其它平台 fp32；1B 版本约需
6 GB 内存。单次请求连同参考音频最多生成约 30 秒，建议使用 3–10 秒的提示音频。
克隆需要文字稿，音频明显长于文字稿时会自动裁剪；数字串会转成中文读法，长文本
会自动分段并以短停顿拼接。

**dots.tts** —— 2B 连续 AR 模型，flow-matching 头部驱动 48 kHz AudioVAE，经
audio.cpp 运行（macOS 用 Metal，其它平台 CPU）。克隆默认的 x-vector 模式只需
参考音频，文字稿不准确也不影响；文字稿准确时「声纹 + 文字」相似度更高。模型常驻
在服务进程里，多次请求之间不必重新加载权重。M4 Pro 上 Q8_0 检查点实测：
RTF 1.39（见「速度」），加载后内存约 6.2 GB。

**CosyVoice3** —— 0.5B 流式 LLM TTS，与 dots.tts 共用 audio.cpp 运行时，和四个
audio.cpp 引擎一样跑在常驻服务里。三种模板：零样本克隆（参考音频 + 准确文字稿，
相似度最高）、跨语种克隆（仅凭声音，文字稿不可靠时使用）和指令控制（例如「用温和
的语气，语速稍慢」）。Top-K 与流解码步数可逐次调整。M4 Pro 上 RTF 0.30，加载后
内存约 3.3 GB。

**Qwen3-TTS** —— 官方 `qwen-tts` 包运行在独立 venv 中。自动选择设备：CUDA 用
bfloat16，Apple MPS 用 float32（codec 上 float16 会产生 NaN），其余走 CPU。
三个安装共用同一 venv：克隆版、`custom`（九个内置说话人：Vivian/Ryan/Sohee 等）
和 `design`（1.7B，按描述创造音色）。请求耗时见上文「速度」。

**Ming-3.0-tts** —— 0.5B 稠密模型，在 Artificial Analysis「受控音色克隆」排行榜
中位列开源模型第一。仅凭参考音频即可克隆，给出文字稿可以提升一致度但不是必需。
四个安装共用同一份权重：克隆、声音设计、多语种（克隆音色但说德/西/法/韩/日）和
内置音色。FST 文本正则需要 `pynini`（仅 conda 提供），缺失时自动跳过。长文本按
约 64 个语义单位分段，拼接前会做响度对齐；普通句子一次生成，不会出现接缝。同一
文本与音色的输出是确定的（使用上游默认种子 1895）。

**Chatterbox** —— Resemble AI 的 0.5B 多语种克隆模型：克隆一段参考音频即可说
23 种语言，不需要文字稿。默认中文，可在引擎参数中选择目标语言。偶尔会在正文结束
后继续生成几秒低电平乱语；引擎会按整段音频的响度裁掉这类长尾，保留 0.3 秒余量。

**IndexTTS-2** —— bilibili 的中英文零样本克隆模型：GPT 式 token 模型以参考音频
（截取前 15 秒）为条件，流匹配声码器输出 22.05 kHz 音频。长文本由模型内部在
句末切成若干段，段与段之间插入约 0.2 秒静音后拼接（audio.cpp 的
`interval_silence_ms`），因此不会出现请求过长导致的退化。不使用文字稿。它运行
在与 dots.tts、CosyVoice3、VoxCPM2 共用的 audio.cpp 原生运行时上，不安装 Python
环境。文本语言自动判断（含汉字按中文，否则按英文），也可逐次指定；Q8_0 包为
3.6 GB，需要更高精度时可用 `INDEXTTS_PACKAGE` 改装 f16 或原始精度包。解码默认
用束搜索，在引擎参数里改成贪心可以快约 25%，读出的文字不变。它和其他 audio.cpp
引擎一样运行在常驻服务里，只有重启后的第一个请求才需要加载 3.6 GB 权重。
M4 Pro 上 Q8_0 检查点实测 RTF 1.47，加载后内存约 4.1 GB（见「速度」）。

**MOSS-TTS-Delay** —— 多语言零样本 TTS / 克隆模型，由原生 llama.cpp 二进制作
推理，运行时无需 Python。安装脚本直接下载预编译运行时（其他平台或设置
`MOSS_RUNTIME_BUILD=1` 时才回退到本地编译），并一次性把音频 tokenizer
转成 GGUF；权重来自 ModelScope，无需登录 Hugging Face。克隆可选。引擎会把长文本
拆成按句切分的片段合成，因为解码器在超长输入下会退化。

该模型只在 macOS（Metal）与 Linux（CPU）上提供。预编译的 Windows 运行时没有 GPU
后端，而且每隔几次运行就会破坏宿主堆内存，因此模型目录不再在 Windows 上提供它。
运行时的 `-ngl` 可以把层卸载到自编译的 CUDA 版本，但在钉住的版本上，所有触碰 GPU
的运行都崩溃了（见「速度」），所以也没有 GPU 这条退路。

**SenseVoice** —— 可选的本地语音识别，覆盖中英日韩粤。安装后，录音或上传的参考
音频会自动转写并填入文字稿栏；「转写」按钮可以手动重跑。

## 速度

测试机：MacBook Pro，Apple M4 Pro（14 核 CPU，10 性能核 + 4 效率核，20 核
GPU），48 GB 统一内存，macOS 27.0.1。

| 引擎 | RTF | 固定开销 | 生成 10 秒语音 |
| --- | --- | --- | --- |
| LongCat-AudioDiT 1B | 0.17 | 8.6 s | 10.3 s |
| ZipVoice | 0.17 | 0.9 s | 2.6 s |
| CosyVoice3 | 0.30 | 0.6 s | 3.6 s |
| MOSS-TTS | 0.33 | 2.4 s | 5.7 s |
| VoxCPM2 | 0.62 | 0.0 s | 6.2 s |
| Kokoro-82M | 0.64 | 0.7 s | 7.0 s |
| Ming 3.0 声音设计 | 0.98 | 4.0 s | 13.8 s |
| Ming 3.0 多语种 | 0.99 | 3.8 s | 13.6 s |
| Ming 3.0 内置音色 | 0.99 | 3.7 s | 13.6 s |
| Ming 3.0 克隆 | 1.00 | 3.8 s | 13.8 s |
| Qwen3-TTS 0.6B 克隆 | 1.00 | 4.3 s | 14.3 s |
| Qwen3-TTS 0.6B 预置音色 | 1.00 | 3.9 s | 13.9 s |
| Qwen3-TTS 1.7B 声音设计 | 1.20 | 4.1 s | 16.1 s |
| dots.tts SOAR | 1.39 | 0.0 s | 13.7 s |
| IndexTTS-2 Q8_0 | 1.47 | 1.1 s | 15.8 s |
| Chatterbox | 3.88 | 9.8 s | 48.6 s |

RTF 是生成耗时除以音频时长，小于 1 表示快于实时。固定开销是请求在产出第一个
采样之前花掉的时间——启动引擎进程、读取参考音频，以及不保留状态的引擎每次
重新加载权重的时间。因此单次请求耗时约为「固定开销 + RTF × 音频时长」，第三列
就是按 10 秒语音算出来的结果。固定开销大而 RTF 低的引擎（LongCat、CosyVoice3）
适合读长文，只说一句话则很浪费。

四个 audio.cpp 引擎（dots、CosyVoice3、VoxCPM2、IndexTTS-2）现在共用一个常驻
的 `audiocpp_server` 进程，首次使用时启动、之后模型常驻，表中的固定开销是热态
数据。应用刚启动后的第一个请求、以及闲置 15 分钟（服务会释放权重）之后的下一个
请求，要额外付一次约 3–11 秒的加载时间。

测试方法：通过本机回环调用服务端接口，用两段中文——一句约生成 2 秒语音，一段
约生成 10–13 秒。每个引擎先发一次预热请求并丢弃，然后短句与长段交替请求至少
两轮，拟合 `T = 固定开销 + RTF × 时长`；每个引擎都从新启动的服务端测起。
MOSS-TTS 与 Ming 声音设计多测了几个点并改用中位数斜率（最小二乘会被单次卡顿
拖偏）。请求串行执行，机器上只有重启后还在补索引的 Spotlight，因此 10% 以内
的差异可以视作噪声。Chatterbox 这一行最粗略：同一段长文一次生成 11 秒、一次
19 秒，RTF 是几次彼此不太一致的运行的中位数。

### Windows 台式机实测

测试机：Intel Core i7-3770K（4 核 8 线程），32 GB 内存，NVIDIA GeForce
RTX 3060 Ti（8 GB 显存），Windows 10 Pro 22H2。

| 引擎 | RTF | 固定开销 | 生成 10 秒语音 |
| --- | --- | --- | --- |
| LongCat-AudioDiT 1B | 0.00 | 40.3 s | 40.3 s |
| CosyVoice3 | 0.18 | 0.6 s | 2.3 s |
| VoxCPM2 | 0.30 | 0.2 s | 3.2 s |
| IndexTTS-2 Q8_0 | 0.60 | 1.0 s | 7.0 s |
| dots.tts SOAR | 0.62 | 0.5 s | 6.7 s |
| Chatterbox | 2.18 | 67.1 s | 88.9 s |
| ZipVoice | 2.45 | 6.8 s | 31.2 s |
| Kokoro-82M | 3.08 | 0.6 s | 31.5 s |
| Ming 3.0 内置音色 | 4.16 | 17.7 s | 59.4 s |
| Ming 3.0 多语种 | 4.25 | 18.7 s | 61.2 s |
| Ming 3.0 声音设计 | 4.71 | 25.8 s | 73.0 s |
| Ming 3.0 克隆 | 5.15 | 24.6 s | 76.1 s |
| Qwen3-TTS 0.6B 克隆 | 5.16 | 22.7 s | 74.3 s |
| Qwen3-TTS 0.6B 预置音色 | 5.28 | 16.8 s | 69.6 s |
| Qwen3-TTS 1.7B 声音设计 | 5.64 | 16.7 s | 73.1 s |
| MOSS-TTS | — | — | — |

四个 audio.cpp 引擎走 CUDA，速度与 M4 Pro 持平甚至更快——dots 0.62 对
1.39，CosyVoice3 0.18 对 0.30，VoxCPM2 0.30 对 0.62，IndexTTS-2 0.60 对
1.47——常驻后的固定开销都在 1 秒以内。ONNX 引擎比 M4 Pro 慢 4–14 倍，因为
这颗 CPU 是 2012 年的四核、没有 AVX2；ZipVoice 6.8 秒的固定开销主要是用它
编码参考音频。Qwen3-TTS 和 Ming 同样慢 4–5 倍：它们的自回归循环受单核 CPU
速度限制，而不是 GPU。

LongCat 显示 0.00 是因为无论文本多长，每次请求都要 40 秒左右——5 秒和 17.6
秒的语音都在约 40 秒内完成——拟合只能看到每次请求的固定成本，即新起 Python
进程重新加载权重；在 3060 Ti 上生成本身远快于实时，但时长项落在加载噪声
之下。

MOSS-TTS 在这台机器上无法测速：预编译的 Windows 运行时没有 CUDA 后端（每层
都分配到 CPU），并且每隔几次运行就会在二进制内部以 0xC0000374（堆损坏）或
0xC0000409 崩溃。它偶尔也能完成——经引擎的一次请求在 171.8 秒后返回了 0.64 秒
音频（含模型加载）——但这样的崩溃率让计时拟合没有意义。引擎现在会对崩溃的进程
最多重试三次；在此之前，Windows 上的重试从未生效，因为崩溃表现为退出码而不是
信号。

同一运行时的 CUDA 构建（CUDA 13.3，arch 86）确实会用 GPU——层和 KV cache 都上了
`CUDA0`——但所有触碰 GPU 的运行都崩溃了：`-ngl -1` 5 次全崩，关闭 CUDA graph
3 次全崩，部分卸载 `-ngl 20` 3 次全崩；而 `-ngl 0` 三次里成功两次。因此模型目录
不再在 Windows 上提供 MOSS-TTS。

即使换用 CUDA 构建，ONNX 引擎仍然跑在 CPU 上：发布模型都是 INT8 导出，
ONNX Runtime 的 CUDA 后端没有对应的量化算子实现，所以 INT8 Kokoro 两种构建
都是 6.3 秒，显存占用始终不变。作为对照，同一模型的 fp32 导出用 GPU 生成
9.6 秒语音只要 1.5 秒，四个 CPU 线程则需要 10.2 秒——GPU 通路本身可用，只是
INT8 模型没有可加速的算子。上表 Kokoro 一行仍是 INT8 的数字。现在 Kokoro
会在这类机器上安装并优先使用 fp32 导出，而且即使不用 GPU，它在这颗 CPU 上也更
快：没有 AVX2 时 INT8 算子退化为标量实现，同一段文字热身后耗时 6.2 秒，INT8
为 14.8 秒。该运行时的构建方法见
[开发文档](development.zh-CN.md#onnx-用-gpu)。

8 GB 显存一次只放得下一个 audio.cpp 模型。让 dots、VoxCPM2、CosyVoice3
同时常驻（约 6.2 + 6.5 + 3.3 GB）时，CosyVoice3 服务在第二次请求时崩溃，
所以上表每一行都是对着新启动的服务端测的。现在应用
在另一个引擎开始合成时会先停掉上一个引擎的常驻服务（从 dots 切到 VoxCPM2
实测释放约 800 MB 显存），同一时间只有一个模型常驻；测速仍按每个引擎各起
一个服务进程进行，让每个引擎都承担相同的启动开销。因此 audio.cpp 那几行是
「热」数据：切回某个引擎时要重新付出模型加载时间——dots 冷启动 44.5 秒，
服务已在运行时只要 11.1 秒。方法同上，只是 LongCat 额外测了三种长度
（5、12.5、17.6 秒语音），因为两点的拟合斜率是负的。

## 环境要求

| 功能 | 要求 |
| --- | --- |
| ONNX 引擎（ZipVoice、Kokoro、SenseVoice） | 除应用外无额外要求 |
| audio.cpp 引擎（dots、CosyVoice、VoxCPM2、IndexTTS-2） | 下载预编译 CLI 与常驻服务 |
| Python 引擎（Qwen3-TTS、LongCat、Ming、Chatterbox） | Python 3.10–3.12；脚本自动创建隔离 venv |
| MOSS-TTS | 仅 macOS/Linux（Windows 上不提供）；下载预编译运行时；一次性转换分词器需要 Python 3 |
| Windows 上应用内安装 | Git for Windows 提供的 `bash` |

需要 Python 的引擎各自运行在独立 venv 中，绝不把代码加载进核心进程。

## 鸣谢

ZipVoice 模型权重为 Apache-2.0，由
[k2-fsa/ZipVoice](https://github.com/k2-fsa/ZipVoice) 发布、
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) 打包。

Kokoro-82M 权重为 Apache-2.0，由 [hexgrad/Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M)
发布；ONNX 导出与打包来自 sherpa-onnx。

VoxCPM2 由 [OpenBMB](https://github.com/OpenBMB/VoxCPM) 以 Apache-2.0 发布；
GGUF 导出与原生运行时来自
[audio.cpp](https://github.com/0xShug0/audio.cpp)。

LongCat-AudioDiT 代码与权重为 MIT，由
[meituan-longcat/LongCat-AudioDiT](https://github.com/meituan-longcat/LongCat-AudioDiT)
发布；UMT5 分词器来自 `google/umt5-base`。

CosyVoice3 代码与检查点为 Apache-2.0，由
[FunAudioLLM/CosyVoice](https://github.com/QwenAudio/CosyVoice) 发布；
GGUF 导出同样来自 audio.cpp。

dots.tts 代码与检查点为 Apache-2.0，由
[studio-dots-ai/dots.tts](https://github.com/studio-dots-ai/dots.tts) 发布。
权重保留其 Apache-2.0 许可，运行时二进制以未修改形式再分发。

Qwen3-TTS 由 [QwenLM](https://github.com/QwenLM) 以 Apache-2.0 发布。

Ming-3.0-tts 由蚂蚁集团发布；许可条款见其模型卡片。

Chatterbox 由 [Resemble AI](https://github.com/resemble-ai/chatterbox) 以 MIT
发布。

IndexTTS-2 由 [bilibili](https://github.com/index-tts/index-tts) 以 bilibili
Model Use License 发布：可免费使用；月活超过 1 亿或年收入超过 10 亿元人民币需
另行申请授权，且不得用于改进其他 AI 模型。GGUF 导出与原生运行时来自 audio.cpp。

MOSS-TTS-Delay 由 [OpenMOSS](https://github.com/OpenMOSS) 以 Apache-2.0 发布；
原生运行时由 OpenMOSS 的 llama.cpp 分支构建。

SenseVoice 由 FunAudioLLM 以 MIT 发布，并由 sherpa-onnx 打包。
