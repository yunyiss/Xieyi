# Xieyi 写意

轻量级本地 AI 翻译工具：**按需加载模型、停手后自动释放显存**，不与前台的其他模型抢资源。

> A local-first AI translation tool that loads models on demand and releases VRAM when idle.

名字取自「写意」——不拘泥于字面形似，重神韵。这正是它区别于传统翻译工具的地方：
译文可以像改稿一样继续打磨，调语意、改内涵、增删内容，而不是翻完就结束。

## 核心设计

- 安装包只含外壳（Tauri v2 + React），llama.cpp 运行时在首次启动时按需下载到应用数据目录
- 常驻的只是 llama-server 的**空壳路由进程**（0 显存），模型作为其子进程按需拉起、用完销毁
- 默认 TTL 卸载（120s），连续翻译命中热调用，停手后自动归还显存
- 完全隔离：随机端口、本地 api-key、不读取系统里的 `OLLAMA_*` / `LLAMA_*` 环境变量

## 三个页面

| 页面 | 作用 |
|---|---|
| **翻译** | 粘贴文本 → 流式译文；语言对、五套风格、术语表、长文分段、质检自检 |
| **批量** | 拖入 txt / md / srt → 生成 `*.目标语言.*`；文件级并发 2、SRT 结构保留 |
| **对话** | 译文的二次加工台：多轮、思维链、消息版本分支、快捷指令 |

## 模型兼容

不同翻译模型的调用方式差异很大，工具会自动识别并用对应的推荐格式：

| 模型家族 | 处理方式 |
|---|---|
| 通用对话模型（Qwen / Llama / Gemma） | 英文 system prompt |
| Hy-MT2、腾讯混元 | 英文 system prompt |
| Index-Translate | 中文文本提示 + 约束格式 |
| TranslateGemma | `chat_template_kwargs` 传语言码 |
| NVIDIA Riva-Translate | system 传语言对 |
| Seed-X | 标记为不可用（官方无对话模板） |

识别不准时可在设置里手动勾选某个模型「能不能用在对话页」。

## 开发

```bash
npm install
npm run tauri dev
```

## 测试

```bash
npm test                    # 前端：Vitest + Testing Library
cd src-tauri && cargo test  # 后端：Rust 单测
```

## 构建

```bash
npm run tauri build
```
