# laverock

[English](README.md) | 中文

本地优先的语音克隆工作室。语音合成与声音克隆全部在你自己的电脑上完成——
不需要云端账号、API Key，也不会上传任何内容。一个桌面应用集成了模型管理、
已保存音色库、生成历史和局域网手机遥控。

## 它能做什么

- **声音克隆与预置音色** —— 保存 3–10 秒参考人声为命名音色，之后任何克隆类引擎
  都能用它朗读新文本；不需要参考音频的场景由预置音色和声音设计模式覆盖。
- **15 个模型，同一套界面** —— 从 135 MB、CPU 上即可实时的 ONNX 小模型，到
  2B 的扩散与 LLM 系统。在应用内安装、更新和卸载，下载进度实时显示。
- **音色库** —— 在应用内录音或上传音频，用本地语音识别自动转写文字稿，之后可以
  编辑或替换音频，并以 `.lvvoice.zip` 导出、导入。
- **历史** —— 每条生成记录都会保留，可回放并下载为 WAV 或 MP3，最新在前。
- **手机遥控** —— 打开局域网访问，扫描二维码，同一 Wi-Fi 下的手机即可使用
  完全相同的界面（HTTPS）。
- **OpenAI 兼容 API** —— 为 OpenAI TTS API 编写的客户端只需修改 base URL。
- **中英双语界面** —— 默认跟随系统语言，可随时切换。

模型下载完成后全部离线运行。支持 macOS、Windows 与 Linux；CPU 推理全平台可用，
较大的引擎在 Apple Silicon 上使用 Metal、在 NVIDIA 显卡上使用 CUDA。

## 快速开始

1. 从 [releases 页面](https://github.com/chen3feng/laverock/releases) 下载对应
   平台的安装包——macOS 仅支持 Apple Silicon。
2. 打开 Laverock，在 **模型** 页选择一个模型。ZipVoice（约 160 MB）体积最小、
   CPU 上最快；Kokoro（约 135 MB）是体验预置音色最轻量的选择。安装包不含模型
   权重，这是唯一需要下载的内容。
3. 在 **语音素材** 页录制或上传参考人声（或按引擎选择预置音色）；在 **合成** 页
   选择引擎与音色、输入文字，点击生成。

应用会把模型、音色和历史保存在系统应用数据目录，并自行托管本地服务端，
不需要额外配置。

## 支持的模型

| 模型 | 模式 | 语言 | 下载体积 |
| --- | --- | --- | --- |
| ZipVoice Distill INT8 | 克隆 | 中英 | 约 160 MB |
| Kokoro-82M INT8 | 54 个预置音色 | 8 种 | 约 135 MB |
| VoxCPM2 | 克隆、声音设计 | 30 种 | 约 3.3 GB |
| LongCat-AudioDiT 1B | 克隆 | 中英 | 约 7 GB |
| dots.tts SOAR | 克隆 | 24 种 | 约 3.1 GB |
| CosyVoice3 0.5B | 克隆、指令控制 | 9 种 + 粤语 | 约 2.3 GB |
| Qwen3-TTS 0.6B / 1.7B | 克隆、预置音色、声音设计 | 10 种 | 2–5 GB |
| Ming-3.0-tts 0.5B | 克隆、预置音色、声音设计、多语种 | 7 种 | 约 3 GB |
| Chatterbox Multilingual | 克隆 | 23 种 | 约 5 GB |
| MOSS-TTS-Delay | 克隆 | 20 种 | 约 10 GB |
| SenseVoice Small（可选） | 语音识别 | 5 种 | 约 234 MB |

各模型的环境要求、注意事项与手工安装脚本见 **[模型](docs/models.zh-CN.md)**。

## 文档

| 文档 | 内容 |
| --- | --- |
| [模型](docs/models.zh-CN.md) | 目录、能力、速度实测、环境要求、手工安装、鸣谢 |
| [语音质量](docs/quality.zh-CN.md) | 各引擎的拟真度、音色相似度、稳定性与准确率实测 |
| [HTTP API](docs/api.zh-CN.md) | REST 接口、示例、OpenAI 兼容 API |
| [手机遥控](docs/remote.zh-CN.md) | 配对、局域网安全、HTTPS、安装为应用 |
| [配置](docs/configuration.zh-CN.md) | 环境变量、数据目录、代理与镜像、界面语言 |
| [开发](docs/development.zh-CN.md) | 从源码构建、启动服务、架构、新增引擎 |

## 许可证

MIT。随包分发与下载的模型权重遵循各自的许可；见
[模型](docs/models.zh-CN.md#鸣谢)。
