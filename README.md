# LocalTranslate

轻量级本地模型翻译工具：**按需加载模型、停手后自动释放显存**，不与前台的其他模型抢资源。

## 核心设计

- 安装包只含外壳（Tauri v2 + React），llama.cpp 运行时在首次启动时按需下载到应用数据目录
- 常驻的只是 llama-server 的**空壳路由进程**（0 显存），模型作为其子进程按需拉起、用完销毁
- 默认 TTL 卸载（90–180s），连续翻译命中热调用，停手后自动归还显存
- 完全隔离：随机端口、本地 api-key、不读取系统里的 `OLLAMA_*` / `LLAMA_*` 环境变量

## 开发

```bash
npm install
npm run tauri dev
```

## 构建

```bash
npm run tauri build
```

## 文档

需求与技术方案位于 `.trae/docs/`（本地文件，不入库）。
