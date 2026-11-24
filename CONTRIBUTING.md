# 参与开发

> 面向要改代码的人。只想用工具的话看 [README](README.md)。

---

## 环境要求

| 项 | 版本 | 说明 |
| :--- | :--- | :--- |
| Node.js | 20+（推荐 22） | 前端构建 |
| Rust | stable | **必须 ≥ 1.85**（项目用 edition 2024） |
| Windows | 10 / 11 | 需要 WebView2 Runtime（Win11 自带）+ MSVC 工具链 |
| Linux | 任一现代发行版 | 需要 `libwebkit2gtk-4.1-dev` 等，见下方 |
| macOS | 13+ | 仅源码构建；需要 Xcode Command Line Tools，未做签名/公证 |

Linux 依赖：

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev \
  patchelf libxdo-dev libssl-dev
```

---

## 快速开始

```bash
npm install
npm run tauri dev      # 自动起 Vite + Tauri 窗口
```

---

## ⚠️ 构建陷阱（必读）

### 必须用 `npx tauri build`，不能用 `cargo build --release`

这是本项目**踩过六次**的坑：

```
cargo build --release
  → 不执行 npm run build
  → 前端 dist/ 不会嵌进二进制
  → 程序启动后去连 http://localhost:1420
  → 没有 Vite 在跑 → 白屏
```

正确做法：

```bash
npx tauri build --no-bundle   # 只出可执行文件，用于验证
npx tauri build               # 出安装包
```

**验证闸门**——构建完必须确认前端真的嵌进去了：

```bash
# Windows
strings path/to/xieyi.exe | grep -cE 'index-[A-Za-z0-9_-]+\.js'   # 必须 ≥ 1

# Linux / macOS
strings path/to/xieyi | grep -cE 'index-[A-Za-z0-9_-]+\.js'      # 必须 ≥ 1
```

返回 0 就是白屏版，别发出去。

### `cargo test` 必须在 `src-tauri/` 下执行

在仓库根目录跑会失败——根目录不是 Cargo workspace。

### 单实例测试的环境依赖

`single_instance::tests::first_acquire_succeeds` 在**已有 `xieyi` 进程运行时必然失败**。
这不是代码 bug，是互斥量语义。跑测试前先关掉正在运行的实例。

---

## 改完代码的验证顺序

```bash
npx tsc --noEmit              # 1. 前端类型
npm test                      # 2. 前端测试（Vitest）
cd src-tauri && cargo test    # 3. 后端测试
npx tauri build --no-bundle   # 4. 需要验证产物时才跑
```

四条全绿才算完成。当前基线：**Rust 192 项 / 前端 116 项（11 个文件）**。

> 这两个数字会随开发变化，读数以 `cargo test` / `npm test` 的实际输出为准。
> 写在文档里只是为了让你一眼看出「是不是少了半个测试套件」。

---

## 架构

```
┌──────────────── WebView2（React 19）────────────────┐
│  App.tsx：三页常驻挂载 + 顶部页签 + 设置抽屉          │
│  lib/ipc.ts：所有 invoke 命令与事件监听集中于此        │
└──────────────────────▲───────────────────────────────┘
                       │ Tauri IPC（命令 + 事件）
┌──────────────────────┴───────────────────────────────┐
│  Rust 后端                                            │
│  service.rs：引擎与模型生命周期、流式补全、翻译流水线   │
│  batch / translate / quality / prompts / runtime /    │
│  models / params / sessions / paths / hardware        │
└──────────────────────▲───────────────────────────────┘
                       │ HTTP 127.0.0.1:<随机端口>，Bearer <本地随机 key>
┌──────────────────────┴───────────────────────────────┐
│  llama-server（路由模式，常驻空壳，0 显存）            │
│    └─ 模型子进程：首次请求时 load，TTL 到期后 unload   │
└──────────────────────────────────────────────────────┘
```

### 三条全局约束

1. **单推理通道** —— 翻译、批量、对话共用一把推理锁，同一时刻只有一个请求在跑
   （避免打爆 llama.cpp 共享的 KV 池）
2. **单模型驻留** —— 换模型就卸载再加载（`--models-max 1`）
3. **一切状态可中断** —— 全局取消位，加载 / 翻译 / 批量 / 对话都在检查点退出并清理子进程

破坏任何一条都会导致难查的问题。改动前先确认不会破坏它们。

---

## 代码布局

```
src-tauri/src/
├── lib.rs              入口、命令注册、启动顺序（日志 → 单实例 → 建窗）
├── service.rs          引擎生命周期、流式补全、翻译流水线（最大的文件）
├── single_instance.rs  单实例守护 + 窗口聚焦（Windows Mutex / Unix flock）
├── job_object.rs       Windows Job Object 进程树回收
├── logging.rs          轻量日志：后台线程、按天分文件、保留 7 天
├── titlebar.rs         系统窗口主题（Tauri set_theme + Windows DWM 补偿）
├── runtime.rs          推理运行时下载、校验、解压
├── models.rs           模型扫描、preset 生成、显存估算
├── paths.rs            各平台数据目录
├── hardware.rs         显存 / 内存查询（注册表 / sysfs / 引擎）
└── ...

src/
├── pages/              Translate / Batch / Chat（三个页面）
├── components/         ThemeMenu、ModelPicker、ContextMeter 等
└── lib/                ipc.ts（IPC 封装）、chat.ts（消息状态机）、useTheme.ts
```

---

## 平台矩阵

| 能力 | Windows | Linux | macOS |
| :--- | :--- | :--- | :--- |
| 编译 | ✅ | ✅ | ✅ CI 验证（仅 arm64） |
| 界面 / 翻译 | ✅ 实测 | ✅ 实测 | ⚠️ Intel x86_64 已实测；Apple Silicon 未实测 |
| 单实例 | 命名 Mutex | `flock` | `flock` |
| 进程树回收 | Job Object | `PR_SET_PDEATHSIG` | ⚠️ 仅进程组 |
| 窗口聚焦 | `EnumWindows` | X11 EWMH | `NSRunningApplication` |
| 标题栏主题 | DWM | GTK | `NSAppearance` |
| 推理后端 | Vulkan / CPU | Vulkan / CPU | Metal / CPU（Intel） |

**Wayland**：协议层面禁止应用获取其他窗口引用，窗口聚焦静默跳过，
行为等同于「第二个实例直接退出」。这是设计，不是缺陷。

**macOS 的进程回收弱点**：没有 `PR_SET_PDEATHSIG`（Linux 专有）。
主进程被强杀时，孤儿进程会留到**下次启动**才被清理。
没在 macOS 上硬造 watchdog——那需要在 `pre_exec` 里起线程，
而 `pre_exec` 只允许 async-signal-safe 操作。

---

## 几个不该改的地方

这些是有意为之，改动前请先读完理由：

| 不要改 | 理由 |
| :--- | :--- |
| **单实例检查移到 `setup` 里** | WebView 在 `Builder::build()` 内部创建，**早于 setup**。放到 setup 里两个实例会同时拿到缓存目录，双双 `ERROR_BUSY` 崩溃 |
| **`system` 主题传 `None`** | 传 `Some(Dark/Light)` 会锁定窗口外观，WebView 的 `prefers-color-scheme` 不再跟随系统 |
| **删掉 Windows 的 DWM 补偿** | `set_theme` 在 `create: false` 手动建窗路径下返回 Ok 但不生效（实测）。这不是冗余代码 |
| **批量并发改成 > 2** | 4 路会打爆共享 KV 池，报 `Context size has been exceeded` |
| **兜底清理按「父进程已退出」判断** | 会误杀用户从终端自己启动的 `llama-server`。判据是命令行里的 api-key |
| **给对话页加 Markdown 渲染** | 已决定不做：对话页是二次加工台，不是通用 Agent |

---

## 提交规范

### 分支

- `main` —— 交付线，每个 commit 都要能拿出去见人
- `dev` —— 开发线，保留完整排查过程

**流程**：功能先提交 `dev` → 确认 → 合并 `main`。

### Commit 格式

Conventional Commits，**英文前缀 + 中文正文**：

```
fix: Windows 标题栏不跟随主题

## 问题
界面切深色后标题栏仍是白色...

## 原因
set_theme 只发 WM_SETTINGCHANGE 消息...

## 验证
截图对比，标题栏色值 #1a1a1a...
```

`main` 上的 commit 写**交付内容 + 技术决策 + 验证结果**；
`dev` 上可以保留试错过程。

---

## CI

`.github/workflows/build.yml` 跑三平台矩阵：

- **test** —— `tsc` + `npm test` + `cargo test`，推 `main`/`dev` 都触发
- **bundle (macOS)** —— 手动触发或打 tag 时构建，并检查前端是否嵌入

macOS 那个 job 的意义是**保证代码路径真的编译过一次**——
之前整个平台连能不能编译都没人知道。

---

## 调试

日志在数据目录的 `logs/` 下，按天分文件，保留 7 天。

| 平台 | 位置 |
| :--- | :--- |
| Windows | `%LOCALAPPDATA%\com.xieyi.translate\logs\` |
| Linux | `~/.local/share/com.xieyi.translate/logs/` |
| macOS | `~/Library/Application Support/com.xieyi.translate/logs/` |

日志格式：`2026-11-05 03:31:08 [INFO] [window] 设置窗口主题: dark`

启动失败会弹原生对话框，里面也带日志路径——因为此时日志模块可能还没初始化。

---

## 许可

贡献的代码以 [MIT License](LICENSE) 授权。

提交即表示你同意 [DCO 1.1](https://developercertificate.org/)：确认你有权提交
该代码，并同意它按本项目协议发布。请在提交时签名：

```bash
git commit -s
```

签名会给每条提交追加一行 `Signed-off-by: 你的名字 <邮箱>`。合并
（squash / rebase）时请保留该行。
