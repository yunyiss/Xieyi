# 第三方代码与许可声明

本项目以 [MIT License](LICENSE) 发布。

下面列出**实际纳入本项目代码**的第三方作品及其许可。仅在阅读层面参考过
设计思路、没有复制代码的项目，列在 README 的「致谢」中。

---

## 1. jan — Apache-2.0

- 来源：<https://github.com/janhq/jan>
- 涉及文件：`src/lib/modelFit.ts`（模型显存 / 内存兼容性判定）
- 说明：判定思路与部分常量参考自 `web-app/src/lib/modelCompatibility.ts`，
  已按本项目需要重写并修改：增加 GGUF KV 结构精确计算、混合注意力层处理
  与中文结论文案。该文件包含基于 jan（Apache-2.0）的衍生部分，相关部分保留 Apache-2.0 声明；本项目其余代码以 MIT 发布。
- 保留声明：This product includes software developed by Menlo Research
  (<https://menlo.ai>).
- 许可全文：<https://www.apache.org/licenses/LICENSE-2.0>

## 2. tauri-cli NSIS 模板 — MIT OR Apache-2.0

- 来源：<https://github.com/tauri-apps/tauri>（`crates/tauri-bundler` 的
  NSIS 模板，v2.12.4）
- 涉及文件：`src-tauri/nsis/installer.nsi`
- 说明：在官方模板基础上增加了「卸载时可选删除 `$INSTDIR\models`」的逻辑，
  文件头已注明来源与改动点。
- 许可：MIT 或 Apache-2.0，任选其一；本项目按 MIT 使用。

## 3. MPL-2.0 依赖（文件级 copyleft，未修改）

以下 crate 以 MPL-2.0 发布，作为**未修改的依赖**使用；对应版本的源码可在
crates.io 获取：

- `cssparser` 0.37.0
- `cssparser-macros` 0.7.1
- `dtoa-short` 0.3.5
- `option-ext` 0.2.0
- `selectors` 0.38.0

MPL-2.0 全文：<https://www.mozilla.org/MPL/2.0/>

## 4. 其他依赖

其余 Rust 依赖均为 MIT / Apache-2.0 / BSD 等宽松许可；前端依赖
（React、React DOM、`@tauri-apps/*` 等）为 MIT 或 Apache-2.0。
完整依赖清单见 `package-lock.json` 与 `src-tauri/Cargo.lock`。

## 5. 运行时下载的组件

推理引擎 [llama.cpp](https://github.com/ggml-org/llama.cpp)（MIT）与模型文件由
用户在首次使用时从官方或镜像源下载，**不随本应用分发**；本项目只提供下载、
校验与启动逻辑。
