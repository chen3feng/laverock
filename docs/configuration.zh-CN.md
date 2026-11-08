# 配置

[English](configuration.md) | 中文 · [← 返回首页](../README.zh-CN.md)

大多数用户不需要这一页：桌面应用会自己选择目录、把设置写进数据目录并托管服务端。
这一页面向无界面部署、自定义安装，以及想把数 GB 的模型挪到另一块硬盘的场景。

## 环境变量

作用于 `laverock-server` 可执行文件。

| 环境变量 | 默认值 | 用途 |
| --- | --- | --- |
| `LAVEROCK_HOST` | `127.0.0.1` | 受信任回环监听的绑定地址 |
| `LAVEROCK_PORT` | `8787` | 端口 |
| `LAVEROCK_DATA_DIR` | `data` | 声音、历史、设置、配对状态、TLS 证书 |
| `LAVEROCK_MODELS_DIR` | `models` | 扫描 `*/engine.json` 的目录；可用 `:`（Windows 为 `;`）分隔多个，靠前的优先 |
| `LAVEROCK_TOOLS_DIR` | — | 引擎使用的 Python venv 与构建工具 |
| `LAVEROCK_FFMPEG` | — | `ffmpeg` 可执行文件，或同时包含 `ffmpeg` 与 `ffprobe` 的目录；用于视频和非 WAV 音频 |
| `LAVEROCK_SCRIPTS_DIR` | 自动检测 | 模型安装器使用的 `scripts/` |

桌面外壳在启动服务端时会设置 `LAVEROCK_DATA_DIR`、`LAVEROCK_MODELS_DIR`、
`LAVEROCK_TOOLS_DIR` 和 `LAVEROCK_SCRIPTS_DIR`。`LAVEROCK_URL` 让外壳连接
另一台机器上的服务端，`LAVEROCK_SERVER_BIN` 指定具体的服务端二进制。

## 目录

| 构建方式 | 数据 | 模型 | 工具（venv） |
| --- | --- | --- | --- |
| 源码运行 | `./data` | `./models` | `./.tools` |
| macOS 应用 | `~/Library/Application Support/sh.laverock.desktop` | `<data>/models` | `<data>/tools` |
| Windows 应用 | `%APPDATA%\sh.laverock.desktop` | `<data>\models` | `<data>\tools` |
| Linux 应用 | `~/.local/share/sh.laverock.desktop` | `<data>/models` | `<data>/tools` |

数据目录的内容：

```text
profiles/     已保存的声音（每个声音一个 WAV 加元数据）
history/      生成的音频，最新在前（最多 100 条 / 1 GiB）
tls/          局域网监听使用的自签证书
settings.json 下载代理、镜像、界面语言、工作区根目录
remote.json   已配对手机（SHA-256 哈希，权限 0600）
```

要把模型与 venv 挪到另一块硬盘，在应用设置里填写 **工作区目录**：模型随后位于
`<工作区>/models`、venv 位于 `<工作区>/tools`。声音和历史仍留在数据目录，因此
工作区不会包含定义它自己的设置。显式设置的工作区也会覆盖
`LAVEROCK_MODELS_DIR` / `LAVEROCK_TOOLS_DIR`。

`LAVEROCK_MODELS_DIR` 列出多个目录时按顺序叠放：同一引擎 id 以靠前的目录为准，
后面的副本在 `GET /api/engines` 中标记为 `shadowed` 且不会被加载；新的安装写入
第一个目录。这样可以把只读的共享权重库与本地覆盖层放在一起。

## 下载网络

**设置 → 下载网络** 保存在 `<data>/settings.json` 中，并传递给每个模型下载脚本；
绝不影响推理。

| 设置 | 环境变量 | 默认值 |
| --- | --- | --- |
| 代理 | `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`（含小写形式） | 系统浏览器代理 |
| Hugging Face 镜像 | `HF_ENDPOINT` | `https://huggingface.co` |
| PyPI 镜像 | `PIP_INDEX_URL` | PyPI |

下载默认跟随浏览器使用的代理：优先使用已导出的 `HTTPS_PROXY`（或 `ALL_PROXY` /
`HTTP_PROXY`），否则读取 macOS 系统代理（`scutil --proxy`）或 Windows Internet
Settings。选择「手动指定」可覆盖，选择「不使用代理」强制直连；手动地址支持
`http`、`https`、`socks5` 与 `socks5h`。不会解析 PAC 脚本——这种情况下请手动
填写代理。脚本运行时会在继承的排除列表外合并 `no_proxy=127.0.0.1,localhost`，
因此本地镜像仍可使用。

## 界面语言

侧栏左下角的语言选择器可在中英文之间切换整套界面：**跟随系统**（按浏览器语言
自动选择）、中文、English。选择保存在本设备；在电脑端显式选择后还会写入
`settings.json` 作为机器默认值，供新配对的手机继承。引擎参数名称、模型目录介绍和
后端校验消息也会随之切换语言。

## 音频

- **历史** 最多保留 100 条或 1 GiB，先到先删；更早的生成会被自动清理。
- **MP3 导出** 由进程内编码器完成（[`rusty_mp3`](https://crates.io/crates/rusty_mp3)，
  Apache-2.0），无需在系统安装 ffmpeg 或 lame。16/24 kHz 的引擎输出按 MPEG-2
  编码；其它采样率会先重采样到九个 MPEG Layer III 合法采样率中最近的一个。
- 生成的 WAV 为单声道 16 位 PCM，采样率取引擎的原生采样率。

## 视频与压缩音频

字幕项目可以直接导入 WAV。视频（MP4、MOV、MKV、WebM 等）和压缩音频（MP3、M4A、
FLAC、OGG 等）需要系统安装 `ffmpeg` 与 `ffprobe`；没有时只能导入 WAV，页面会给出
提示。查找顺序为：

1. **设置 → ffmpeg 路径** —— 可执行文件本身，或同时包含两个二进制的目录。支持
   `~` 展开；路径必须是绝对路径。
2. `LAVEROCK_FFMPEG` —— 含义相同。
3. `PATH`。

上传会流式写入磁盘，读取时长与画面尺寸后，把第一条音轨转换为单声道 48 kHz WAV。
`GET /api/health` 返回 `"ffmpeg": true|false`；`GET /api/settings` 还会返回解析到的
`ffmpeg_bin`，供设置页显示实际找到的路径。

## 打包版

打包后的桌面版在应用包内附带 `scripts/`（macOS 为 `Contents/Resources/scripts`，
Windows/Linux 在可执行文件旁），下载内容、构建工具和 venv 放在用户数据目录，
因此已安装的应用不会读取源码目录。Windows 上应用内安装模型需要 Git for Windows
提供的 `bash`；下载脚本在各平台其余部分完全一致。
