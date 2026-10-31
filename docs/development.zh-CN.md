# 开发

[English](development.md) | 中文 · [← 返回首页](../README.zh-CN.md)

这一页面向构建与扩展 Laverock 的开发者。如果你只是想使用应用，
看 [README](../README.zh-CN.md) 就够了。

## 环境要求

- Rust stable（edition 2021）与 Cargo。
- 仅 Linux：Tauri 的 WebView 依赖 —— `libwebkit2gtk-4.1-dev`、
  `libappindicator3-dev`、`librsvg2-dev`、`patchelf`、`libgtk-3-dev`。
- 应用本身不需要 C++ 工具链与 CMake。只有给未发布预编译包的平台
  现场编译 MOSS-TTS 运行时（`MOSS_RUNTIME_BUILD=1`）时才需要。

## 从源码运行

```sh
cargo run -p laverock-server     # 然后打开 http://127.0.0.1:8787
```

服务端从 `crates/laverock-server/assets/` 提供内嵌的 Web 界面。默认环境下它使用
`./data` 与 `./models`，因此一个源码检出会拥有自己的声音、历史和模型。

桌面外壳：

```sh
./scripts/run-desktop.sh
```

脚本会构建 `laverock-desktop` 与 `laverock-server`。在 macOS 上它把二进制包进
一个最小 `.app` 并做 ad-hoc 签名，因为麦克风访问（`getUserMedia`）需要带
`NSMicrophoneUsageDescription` 的 bundle 和独立的 TCC 身份——从终端直接启动裸
二进制会被归到终端名下，通常无声地被拒绝。

外壳会托管服务端：如果 `127.0.0.1:8787` 没有响应，它会自行启动
`laverock-server`（并把数据、模型、工具目录传给它），然后打开同一套 Web 界面的
窗口。关窗只是隐藏；托盘菜单提供显示/隐藏、开机自启、打开数据目录和退出。
`LAVEROCK_SERVER_BIN` 可指定具体的服务端二进制，`LAVEROCK_URL` 可连接其它机器
上的服务端。

## ONNX 用 GPU

ZipVoice、Kokoro、SenseVoice 默认跑在 CPU 上：默认构建链接的是静态、仅 CPU 的
sherpa-onnx 运行时。CUDA 构建改为链接共享运行时，并请求 `cuda` 执行后端：

```sh
./scripts/setup-onnx-cuda.sh   # 下载运行时并打印 SHERPA_ONNX_LIB_DIR
SHERPA_ONNX_LIB_DIR=<上面打印的路径> \
  cargo build -p laverock-server --no-default-features --features onnx-cuda
```

运行时需要 PATH 上有 CUDA 13 与 cuDNN 9 的 DLL；装了 PyTorch 的话一般已经在
`Lib/site-packages/torch/lib` 里。如果驱动没有可用设备或缺少 cuDNN，各引擎会打一条
警告并留在 CPU 上——后端启动失败会直接终止进程，所以检查放在创建会话之前。
CUDA 构建下也可用 `LAVEROCK_ONNX_PROVIDER=cpu` 强制走 CPU。

只有非量化模型能受益：CUDA 后端没有随发布模型导出的 INT8 算子实现，量化模型在两种
构建下一样快。在 RTX 3060 Ti 上，fp32 Kokoro 生成 9.6 秒语音用 1.5 秒，四个 CPU
线程要 10.2 秒；而 INT8 Kokoro 两种构建都是 6.3 秒。

audio.cpp 也打包了 Kokoro（`kokoro_tts`），但没有采用：在 RTX 3060 Ti 上两者
GPU RTF 相同（0.152 对 0.156），而 ONNX 导出在 CPU 上更快（1.06 对 1.89），
加载约 1 秒对约 95 秒，下载 325 MB 对 889 MB。Kokoro 继续走 ONNX。

因此安装脚本按「运行安装的那台机器」而不是构建机器选择文件：服务端导出
`LAVEROCK_ONNX_VARIANT`（`fp32` 或 `int8`，由运行时后端与本机 AVX2 检测决定），
`download-kokoro.sh` 读到 `fp32` 时会在 INT8 之外再下载 fp32 导出。手动运行脚本时
它会自行做同样的检测；`LAVEROCK_ONNX_VARIANT` 或 `KOKORO_FP32=1/0` 可覆盖。
加载时引擎在本机偏好 fp32 且清单里的 `model_fp32` 文件存在时使用它，只装了 INT8
时打一条警告。

## 架构

```text
laverock-core     引擎 trait + 清单注册表 + ONNX/GGUF 运行时
laverock-server   轻量 HTTP 控制面 + 内嵌单页应用 + 模型安装器
laverock-desktop  Tauri 外壳：托盘、开机自启、服务托管
```

`laverock-core` 把引擎目录转换成由清单 `kind` 选出的 `SpeechEngine` 实现。ONNX
引擎（ZipVoice、Kokoro、SenseVoice）在进程内使用官方
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) Rust crate 运行；GGUF 引擎
通过 audio.cpp 或 llama.cpp 二进制执行；Python 引擎由 `scripts/` 中的运行器以
子进程方式驱动，各自运行在独立 venv 中。

服务端负责声音、历史、设置、配对和模型目录。单页应用是纯 HTML/CSS/JS，由
`include_str!` 编译进服务端。源码在 `crates/laverock-server/web/`：一个外壳
`index.html`、样式表、页面标记，以及按功能分节的多个 JavaScript 文件；
`python3 scripts/build-web.py` 把它们拼成最终发布的单个 `assets/index.html`
（运行时无依赖、不上打包器）。改 `web/` 下的文件、跑脚本、再重新编译服务端；
CI 会执行 `build-web.py --check`，产物落后于源码的 PR 合不进去。

## 新增模型

受支持类型的模型不需要改代码：创建 `<models>/<id>/engine.json`，把其中的路径指向
该目录里的文件即可。受支持的 `kind` 有 `zipvoice`、`voxcpm2`、`qwen3tts`、
`longcat`、`dots`、`cosyvoice`、`kokoro`、`ming3tts`、`chatterbox`、
`indextts2`、`mosstts` 和 `stt`。

```json
{
  "id": "kokoro-int8-multi-lang-v1_0",
  "kind": "kokoro",
  "name": "Kokoro 82M 多语言 (INT8)",
  "languages": ["zh", "en", "ja"],
  "capabilities": ["tts", "preset-voices"],
  "kokoro": {
    "model": "model.int8.onnx",
    "voices": "voices.bin",
    "tokens": "tokens.txt",
    "data_dir": "espeak-ng-data",
    "length_scale": 1.0,
    "num_threads": 4
  },
  "defaults": { "speed": 1.0, "num_steps": 4 },
  "parameters": [
    {
      "name": "voice",
      "label": { "en": "Voice", "zh": "音色" },
      "type": "choice",
      "options": [{ "label": "Heart (af_heart)", "value": "3" }],
      "default": "3"
    }
  ]
}
```

- `capabilities` 决定界面行为（`clone`、`preset-voices`、`voice-design`、
  `instruct`、`multilingual`、`stt`）。
- `parameters` 就是整套控件：`name`、`label`、`type`（`float` / `int` / `bool` /
  `choice`）、`min`、`max`、`step`、`options`（label/value 对）、`default` 和
  `hint`。合成页据此生成控件，服务端按同一份列表校验提交值。未声明时使用各类型
  的内置参数。
- `label`、`hint`、`name` 与目录介绍使用 `{"en": …, "zh": …}` 对象，双语文案才能
  随界面切换；手写的第三方清单也可以直接写字符串。
- `Manifest::problem()` 会在用户遇到合成错误之前标记安装的结构性问题（例如来自
  旧运行器的安装）。

要让模型出现在界面的可安装目录中，在
`crates/laverock-server/assets/catalog.json` 里加一条记录，包含 `id`、`dir`、
本地化的 `name`/`description`/`note`、`size`、`languages`、`capabilities` 和
负责安装的 `script`。

## 安装脚本

模型安装脚本位于 `scripts/download-*.sh`，界面与命令行共用。它们可重复执行，
读取 `LAVEROCK_MODELS_DIR`、`LAVEROCK_TOOLS_DIR`、`HF_ENDPOINT`、`PIP_INDEX_URL`
和代理变量，并输出服务端用于进度条解析的进度行。公共辅助函数（下载、venv、
Python 探测）在 `scripts/laverock-lib.sh` 中。各引擎的变体见
[模型](models.zh-CN.md#安装)。

## 测试

```sh
cargo fmt --all -- --check
# onnx-cpu（默认）与 onnx-cuda 互斥，因此不用 --all-features。
cargo clippy --all-targets -- -D warnings
cargo test --workspace
./scripts/check-shell.sh          # bash 3.2 兼容性 + shellcheck

# 需要已安装 ZipVoice + SenseVoice 并启动服务端
./scripts/smoke-test.sh           # 功能面扫描
./scripts/e2e-test.sh             # 录音 -> 转写 -> 合成 -> 导出
```

CI 会在 macOS、Linux、Windows 上运行以上检查，然后在 Ubuntu 上用较小的 ZipVoice
模型与 SenseVoice 启动服务端，执行 `scripts/smoke-test.sh` 和 `scripts/e2e-test.sh`。推送 `v*` 标签（或手动触发 Release
workflow）会构建 macOS 与 Windows 安装包并附加到 release。
