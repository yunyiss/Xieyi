<div align="center">

<img src="assets/brand/swallow_512.png" width="104" alt="Xieyi" />

# Xieyi 写意

</div>

---

<p align="center"><i>A local GGUF translation tool — models load on demand, VRAM returns when you stop.</i></p>

<p align="center">
  <img alt="License" src="https://img.shields.io/badge/License-MIT-3DA639">
  <img alt="Platform" src="https://img.shields.io/badge/Platform-Windows%20%7C%20Linux-0078D4">
  <img alt="Shell" src="https://img.shields.io/badge/Shell-Tauri%20v2-24C8DB">
  <img alt="UI" src="https://img.shields.io/badge/UI-React%2019-61DAFB">
  <img alt="Core" src="https://img.shields.io/badge/Core-Rust-000000">
  <img alt="Engine" src="https://img.shields.io/badge/Engine-llama.cpp-8957E5">
  <img alt="Privacy" src="https://img.shields.io/badge/Privacy-100%25%20Offline-brightgreen">
</p>

<p align="center">
  <img alt="Models" src="https://img.shields.io/badge/Models-GGUF%20Local-orange">
  <img alt="VRAM" src="https://img.shields.io/badge/VRAM-idle%20released-yellowgreen">
  <img alt="Install" src="https://img.shields.io/badge/Install-3%20MB%20shell-blueviolet">
</p>

<p align="center">
  <b>Fully offline. No cloud API, no telemetry.</b><br />
  The name comes from <i>xiěyì</i> (写意) — Chinese ink painting that favors the spirit over literal likeness.<br />
  A translation doesn't end when the text is rendered; you can keep refining it like a draft.
</p>

<p align="center">
  <a href="#download">Download</a> ·
  <a href="#first-run">First Run</a> ·
  <a href="#the-three-pages">The Three Pages</a> ·
  <a href="#choosing-a-model">Choosing a Model</a> ·
  <a href="#where-files-live">Where Files Live</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="#contributing">Contributing</a>
</p>

<p align="center"><sub><b>English</b> | <a href="README.md">简体中文</a></sub></p>

---

> [!IMPORTANT]
> **No network, no daemon, no resource hogging.**
> What stays resident is only the engine's **empty router process** (0 VRAM).
> Models are spawned on first request and unloaded after 300 seconds (5 min) idle,
> returning VRAM to the system.
> Apart from the one-time engine download, the app makes no network requests.

---

## What it is

In one line: **it turns a `.gguf` file into a usable translation tool.**

No command line, no OpenAI-compatible endpoint to configure, no documents
uploaded anywhere. Install the shell, drop in a model, translate.

**It does exactly three things:**

| Page | What it does |
| :--- | :--- |
| **Translate** | Paste text, get a translation. Quality checks flag suspicious segments and retry them |
| **Batch** | Drop in `txt` / `md` / `srt` files, get translations written next to the originals |
| **Chat** | A workbench for polishing translations: adjust nuance, tone, terminology |

**It is not**: a cloud translation service, a general chat client, a model
manager, or a multi-model concurrent server.

---

## Download

| Platform | Status | How to get it |
| :--- | :--- | :--- |
| **Windows 10 / 11** | ✅ Tested | `Xieyi_x.y.z_x64-setup.exe` from [Releases](https://github.com/yunyiss/Xieyi/releases) |
| **Linux (x64)** | ✅ Tested (X11; window focus unavailable on Wayland) | `Xieyi_x.y.z_amd64.deb` or `.rpm` |
| **macOS (Apple Silicon)** | ⚠️ **Builds from source; runtime unverified** | No prebuilt binaries — build from source |

> **About macOS**: there are complete macOS code paths (single instance,
> `NSRunningApplication` focus, `ps`-based cleanup, Metal backend) and CI
> compiles it on real hardware. But **nobody has run the full flow on a Mac**,
> so no binaries are published and no support is claimed.
> If you try it and report back, that would be genuinely useful.

**The installer is ~3 MB** because it ships neither the inference engine nor
any model — both are fetched on first run. The engine download is ~30 MB and
is SHA-256 verified.

---

## First Run

A **full-screen onboarding layer** appears on first launch. Two steps and
you're done. You can also hit "configure later" to look around first.

### Step 1: Get the engine

The app scans your disk for an existing `llama-server` (from Ollama, LM Studio,
etc.), lets you point it at another path, or downloads one.

> **Downloading is the recommended path.** The default is the **Vulkan** build
> because a single package covers NVIDIA / AMD / Intel and pure CPU — you don't
> have to know what GPU you have. Translation models are small; Vulkan is
> more than fast enough.

After downloading it verifies the SHA-256 and runs a `--version` self-check to
confirm the engine actually starts.

### Step 2: Add a model

Drop a `.gguf` into the `models` folder **next to the executable**, point the
app at an existing model directory, or drag the `.gguf` straight into the window.

> If the program directory isn't writable (e.g. installed under
> `Program Files`), it falls back to `Documents\Xieyi\models`.

The onboarding layer disappears once configured.

---

## The Three Pages

### Translate

Paste → streaming translation.

- **14 languages**, Simplified and Traditional listed separately
- **Glossary**: pin how specific terms must be rendered, applied throughout
- **Long-text segmentation**: oversized input is split and translated piecewise
- **Quality check**: self-review after translating; suspicious segments are
  retried automatically. The footer shows `checked 2/12 · all corrected`
- **Send to Chat**: carry the result over for polishing

<p align="center">
  <img src="assets/screenshots/translate.png" width="720" alt="Translate page">
  <br />
  <sub>Source on the left, translation on the right; streaming output, engine port and residency in the footer</sub>
</p>

### Batch

Drop files in and they queue up.

- `txt` / `md` / `srt`; **SRT timing and structure preserved exactly**
- Detects `GB18030` automatically (common in older Chinese files)
- Output written as `original.target-language.txt`, **never overwriting** existing files
- Failed segments retry individually; one failure doesn't abort the file
- Summary of successes and elapsed time at the end

> **Concurrency is capped at 2.** Not caution — measured: 4-way parallelism
> blows out llama.cpp's shared KV pool and fails with
> `Context size has been exceeded`.

<p align="center">
  <img src="assets/screenshots/batch.png" width="720" alt="Batch page">
  <br />
  <sub>Drop files to queue; output lands next to the source; success count and timing at the end</sub>
</p>

### Chat

The **workbench** for a translation. Bring the text over and keep working.

- Multi-turn, with **live reasoning display** and the ability to review it later
- **Message version branches**: regenerating keeps the old version so you can flip back and compare
- Edit-and-resend, delete individual messages
- Sessions persisted locally, surviving restarts
- Context usage ring with a compaction prompt when it fills up
- Quick commands that fold standing instructions (e.g. "more formal") into a message

<p align="center">
  <img src="assets/screenshots/chat.png" width="720" alt="Chat page">
  <br />
  <sub>Workbench for translations, with quick commands and a context usage ring</sub>
</p>

---

## Choosing a Model

**Any of them works.** Drop a model in and it will be recognized; anything
unrecognized is treated as a general chat model — a model never disappears
from the UI just because the app couldn't classify it.

Translation models differ wildly in how they want to be fed: some need an
English system prompt, some only accept a Chinese template, some need the
language code passed through a special parameter. The app reads the GGUF's
`chat_template` metadata **plus the filename** to decide, and **maintains no
model list** (new models ship monthly; a list would always lag).

| Family | Mechanism | Notes |
| :--- | :--- | :--- |
| General chat models (Qwen / Llama / Gemma…) | English system prompt | Fallback path; works for anything |
| Index-Translate | Chinese text prompt | With a hard-constraint block |
| TranslateGemma | Template kwargs / delimiter | Traditional Chinese must use `zh-TW` |
| NVIDIA Riva-Translate | Language pair | Passed directly as `zh-en` in system |
| **Hy-MT2 / Tencent Hunyuan** | Official Chinese template | **Rejects an English system prompt** |
| Seed-X | — | No official chat template; marked unavailable |

> **Translation-specific models must be fed in their trained format.** This was
> learned the hard way: Hy-MT2-1.8B returns short words unchanged and always
> gives Simplified when asked for Traditional if it receives an English system
> prompt. With the official Chinese template everything behaves. If a
> translation model acts strange, check its official model card first.

**Size guidance**: 1.8B runs, 7B and up is noticeably steadier.
`Hello world`-style micro-inputs may still come back unchanged on a 1.8B model
— the quality check flags this. That's the model's ceiling, not a bug.

### Wrong guess? One checkbox in Settings

Detection reads the filename plus the `chat_template` fingerprint, and
**maintains no model-name list**. That has a necessary cost: rename
`Hy-MT2-7B.gguf` to `mymodel.gguf`, or use some obscure model nobody has seen,
and it gets treated as a **generic chat model** — it still translates, just not
through the best call format for that family.

You don't have to rename files or dig through documentation. Open
**Settings → Models & Glossary**; each model row has two checkboxes:

| Checkbox | Effect |
| :--- | :--- |
| **Chat** | Checked = allowed on the Chat page; unchecked = Translate/Batch only |
| **Pinned** | Checked = never auto-unloaded, keeps holding VRAM while idle |

A manually-changed value shows a `·manual` marker, meaning your choice wins over
detection. **To go back to automatic**: set the checkbox back to whatever
detection would have chosen (e.g. if detection said "chat-capable", uncheck then
re-check it) — the override record is cleared and detection takes over again.

> **Why this escape hatch exists**: detection will be wrong sometimes (renamed
> files, obscure models, new quantization variants). Leaving the user stuck, or
> forcing them to rename model files, is not acceptable.
>
> **The default leans toward "recognize rather than hide"**: anything
> unrecognized is treated as a generic chat model and never disappears from the
> UI. The worst case is a suboptimal prompt, never "my model is gone".

---

## Where Files Live

Everything is under one data directory, sharing a root with the WebView2 cache.
Nothing is written to `Roaming`.

| Platform | Data root |
| :--- | :--- |
| Windows | `%LOCALAPPDATA%\com.xieyi.translate` |
| Linux | `~/.local/share/com.xieyi.translate` |
| macOS | `~/Library/Application Support/com.xieyi.translate` |

Inside:

| Content | Location |
| :--- | :--- |
| User config (incl. engine info) | `<data root>/config.json` |
| Session history | `<data root>/sessions/<id>.json` |
| Logs (7-day retention) | `<data root>/logs/xieyi-YYYY-MM-DD.log` |
| Downloaded engine | `<data root>/runtime/llama-<version>-<backend>/` |

> **Deleting the data root = factory reset.** Model directories live elsewhere
> and are unaffected.

### What uninstall leaves behind

- **Installer build** (on Windows it installs to `%LOCALAPPDATA%\Xieyi`, no
  admin rights needed): the installer and uninstaller UI is Simplified
  Chinese, with two checkboxes, **both unchecked by default**:
  - **删除应用程序数据** (delete app data): removes the data root (config,
    sessions, downloaded engine).
  - **删除程序目录中的模型文件** (delete model files in the app folder):
    removes only `.gguf` files under the app's own `models/`; if that leaves
    the install folder empty, the folder is removed as well. Model directories
    you picked in Settings are not touched — the uninstaller does not know
    where they are, and must not guess.
  - Both unchecked: data and models are fully kept for a future reinstall.
  - Silent uninstall (`/S`) skips the UI and behaves like "both unchecked".
- **Portable build**: there is no install/uninstall step — data and models live
  in `data/` and `models/` beside the executable. Delete the folder to remove
  everything; nothing is left in system directories.

---

## FAQ

**Q: Double-clicking does nothing / I get "resource in use"?**
A previous crash left the WebView cache locked. Wait a few seconds and retry;
or end all `msedgewebview2.exe` processes in Task Manager; or reboot once.

**Q: Translation is slow.**
First check that it's actually using the GPU — the settings drawer shows the
engine backend and residency state. Pure CPU works but is far slower; a 1.8B
model on CPU manages tens of tokens per second.

**Q: The first translation takes a few seconds.**
The model has to be loaded from disk and warmed up on first use. To skip that
wait, enable "Preload model at startup" under Settings → Engine: startup takes
a few seconds longer and claims VRAM immediately, but the first translation is
nearly instant afterwards. Off by default.

**Q: VRAM is full / I want it released sooner.**
Idle unload defaults to 300 seconds (5 min). Adjust the TTL in settings, or unload manually.

**Q: I closed the app but VRAM wasn't released.**
That shouldn't happen — Windows uses a Job Object and Linux uses
`PR_SET_PDEATHSIG`, so the kernel terminates the whole process tree when the
main process dies. If you hit this, send the logs (`<data root>/logs/`) —
it's a bug worth fixing.

**Q: Can I open two windows?**
No, single instance. Clicking the icon again brings the existing window forward.

**Q: Does it support AMD / Intel GPUs?**
Yes, via Vulkan. VRAM is queried straight from the engine
(`llama-server --list-devices`), which reports free memory for CUDA / Vulkan /
ROCm / Metal alike, so automatic layer placement works on all of them. If that
is unavailable (older engine, a non-llama.cpp program you pointed at), it falls
back to `nvidia-smi`, and only then to "all layers on GPU".

**Q: Why no Markdown rendering?**
The Chat page is a **post-editing workbench**, not a general agent. Full
HTML/Markdown rendering adds security and performance complexity that
conflicts with the "light and fast" positioning.

---

## Known Limitations

- **Engine version is pinned**: follows a fixed llama.cpp build number rather than `latest`, to avoid behavior drift
- **Model detection depends on filenames**: renaming `Hy-MT2-7B.gguf` to `mymodel.gguf` makes it a general model
- **Batch concurrency capped at 2**: 4-way triggers `Context size has been exceeded`
- **VRAM estimation errs low on purpose**: when both the engine and `nvidia-smi` report a figure, the smaller one wins — a few layers fewer (slower) beats an out-of-memory load failure
- **No window focus on Wayland**: the protocol forbids apps from manipulating other windows — a design choice, not a missing feature

---

## Contributing

See **[CONTRIBUTING.md](CONTRIBUTING.md)** — architecture, build pitfalls,
verification gates, platform matrix.

The short version:

```bash
npm install
npm run tauri dev
```

> ⚠️ Always build with **`npx tauri build`**, never `cargo build --release`.
> The latter doesn't bundle the frontend, so the app tries to reach a
> non-existent dev server and shows a blank window.
> Details in CONTRIBUTING.

---

## Acknowledgements

Design ideas drawn from these excellent open-source projects (**design and
usage only, no code copied**):

| Project | License | What was borrowed |
| :--- | :--- | :--- |
| [llama-swap](https://github.com/mostlygeek/llama-swap) | MIT | Process-tree cleanup, load progress, TTL and router config |
| [STranslate](https://github.com/STranslate/STranslate) | MIT | Translation-source interface, prompt sets, back-translation idea |
| [jan](https://github.com/janhq/jan) | Apache-2.0 | Tauri v2 project layout, message state model |
| [LinguaGacha](https://github.com/neavo/LinguaGacha) | Unspecified | Batch state machine, quality rules (ideas only) |
| [pot-desktop](https://github.com/pot-app/pot-desktop) | GPL-3.0 | Hotkey and tray design ideas (**no code copied**) |

The inference core is [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT).

---

## License

Released under the [MIT License](LICENSE).

---

<p align="center">
  <sub>Xieyi 写意 · local-first, quiet, accurate</sub><br />
  <sub><a href="README.md">简体中文</a> · <a href="CONTRIBUTING.md">Contributing</a></sub>
</p>
