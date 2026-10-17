# Running PDF Translate on Windows

The application is developed on Linux, where ONNX Runtime arrives as part of a
sherpa-onnx bundle and is adopted automatically. On Windows nothing provides
that runtime, so the pieces below have to be put in place once.

Everything in this guide is under the repository folder (referred to as the
*repo* below) unless it says otherwise, so have a checkout of the project
first (`git clone` it, or unpack a source archive).

## Quick start

If Rust (MSVC) and the NVIDIA driver are already installed, two commands run
from the repo root are enough — the first is only needed once:

```powershell
powershell -ExecutionPolicy Bypass -File tools\fetch-ort-windows.ps1
cargo run --release -- "C:\path\to\document.pdf"
```

The sections below explain each piece, how to verify it, and what to do when
something does not work.

## What is needed

| piece | why | where it goes |
| --- | --- | --- |
| 64-bit Windows 10 or 11 (x64) | the ONNX Runtime and CUDA builds are `win-x64`; ARM64 is not supported | system |
| PowerShell 5.1 or newer | runs the installer script (default on Windows 10/11) | system |
| Internet access | downloads the runtime (once) and the models (once) | — |
| Rust ≥ 1.92, `x86_64-pc-windows-msvc` | build the app | `%USERPROFILE%\.cargo` / `%USERPROFILE%\.rustup` |
| Visual Studio Build Tools (C++ workload) | `link.exe` and the Windows SDK that `rustc` uses | `C:\Program Files (x86)\Microsoft Visual Studio` |
| NVIDIA driver for CUDA 12.x or newer | the CUDA execution provider talks to the driver, not to a toolkit | system |
| ONNX Runtime **GPU** build | runs the Qwen2.5 model | `ort\onnxruntime-win-x64-gpu-<version>\lib\` |
| CUDA runtime, cuBLAS, cuFFT, cuDNN 9 | imported by `onnxruntime_providers_cuda.dll` | the same `lib\` folder |
| Qwen2.5 ONNX model (~1.2 GB) | the translator | `%USERPROFILE%\transcribe-models\onnx` by default |

What is **not** needed: the CUDA Toolkit installer, the cuDNN SDK installer, a
Conda environment, or Python. The runtime setup needs no administrator rights
(installing the toolchain may ask for them). The CUDA libraries come from the
PyPI runtime wheels, and the runtime is a plain zip.

The GPU is not optional: the session is always created with the CUDA provider
(`CPU` is only the fallback provider for the few nodes CUDA does not
implement). A CUDA-capable NVIDIA GPU with a current driver is therefore
required. With less than 6 GB of VRAM the app picks the int4 export (about
1.2 GB of weights plus the KV cache, ~2 GB while translating); with 6 GB or
more it downloads the fp16 export of the default `qwen2.5:1.5b` instead
(about 3.1 GB). `qwen2.5:3b` has no fp16 export and always runs int4 — it does
not fit comfortably in 4 GB.

Budget roughly 1.8 GB of downloads for the runtime, 1.2 GB for the model, and
about 2.4 GB of disk for the installed runtime plus a `target\` folder of
about 1 GB after the first build.

## Why the runtime has to be installed by hand

Windows already contains an `onnxruntime.dll` in `C:\Windows\System32`
(Windows ML / WebView2) and it is on the default DLL search path, so ONNX
Runtime's loader finds it. That copy is old: it reports

```
Unsupported model IR version: 10, max supported IR version: 9
```

while the Qwen2.5 exports are IR version 10. The fix is not to disable the
system copy, but to give the application a current runtime to load instead —
it searches `ort\` first and only falls back to the system library when that
folder is missing (see `prepare_runtime` in `src/llm.rs`).

A current runtime alone is not enough: the CUDA provider DLL and the CUDA
libraries have to sit next to `onnxruntime.dll`, because the loader resolves a
DLL's dependencies through the executable's folder and `PATH`, not through the
folder the DLL was loaded from. The app compensates with
`SetDllDirectoryW` (`preload` in `src/llm.rs`), which is why everything must be
in one directory.

`onnxruntime_providers_cuda.dll` imports all of these DLLs at load time, so
Windows needs every one of them to be resolvable before the provider can even
be registered — even cuFFT, which translation itself never uses. A missing DLL
means no CUDA provider, and the session would otherwise fall back to the CPU
(the app reports that as an error naming the missing DLL).

## 1. Toolchain

Install [rustup](https://rustup.rs/) with the default **MSVC** toolchain, or
make an existing one the default:

```powershell
rustup default stable-msvc
rustup update stable
rustc --version            # 1.92 or newer
rustup show active-toolchain   # should end in -msvc
```

If `cargo build` complains that `link.exe` was not found, install Visual Studio
Build Tools with the *Desktop development with C++* workload (the rustup
installer offers this) and start a new terminal. The *Windows 10/11 SDK* and
*MSVC v143* components are the ones that matter. Non-interactively:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

A toolchain older than 1.92 fails with a `rust-version` error, because the
renderer crate (`hayro`) requires it — `rustup update stable` fixes that.

## 2. GPU driver

The CUDA libraries are built against CUDA 12.x and talk to the driver through
`nvcuda.dll`, so only the driver is needed, not the toolkit. Verify it:

```powershell
nvidia-smi
```

Any current Game Ready / Studio driver works; the CUDA 12.x minimum on Windows
is 527.41, and newer drivers (which report CUDA 13.x) run CUDA 12.x binaries
unchanged. If `nvidia-smi` is missing or reports an old driver, install the
current driver from NVIDIA first.

The app reads the total VRAM from `nvidia-smi` to choose int4 or fp16; if the
command is not on `PATH` it assumes a small card and uses int4, which works
everywhere.

## 3. ONNX Runtime and the CUDA libraries

From the repo root, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -File tools\fetch-ort-windows.ps1
```

The script downloads and unpacks, into `ort\`:

```
ort\
├── .downloads\                                    (archives, for re-runs)
└── onnxruntime-win-x64-gpu-1.22.0\lib\
    ├── onnxruntime.dll
    ├── onnxruntime_providers_cuda.dll
    ├── onnxruntime_providers_shared.dll
    ├── cudart64_12.dll
    ├── cublas64_12.dll
    ├── cublasLt64_12.dll
    ├── cufft64_11.dll
    └── cudnn*_9.dll  (eight DLLs; cuDNN 9 is split into separate modules)
```

(`onnxruntime_providers_tensorrt.dll` also comes with the zip and is unused.)

It is idempotent: anything already in place is skipped, and `-Force`
re-downloads. `-OrtVersion`, `-CudaVersion`, `-CublasVersion`,
`-CufftVersion` and `-CudnnVersion` override the defaults. `ort\` is
git-ignored; the installed folder is about 2.4 GB, and the archives (about
1.8 GB) are kept in `ort\.downloads` for re-runs — delete that folder once
the runtime is in place if the space is needed.

The script uses `curl.exe` when it is available (Windows 10 1803+) and
`Invoke-WebRequest` otherwise. No administrator rights are required.

Keep the ONNX Runtime release and the CUDA wheels on matching lines: an
ONNX Runtime built for CUDA 12.x / cuDNN 9 with a CUDA 13 or cuDNN 8 wheel
will fail to load the provider. A newer driver is fine — a CUDA 12.9 build runs
on a driver that reports CUDA 13.x.

### Doing it by hand

If the script cannot be used, the same five downloads can be fetched
manually:

1. [ONNX Runtime releases](https://github.com/microsoft/onnxruntime/releases)
   → `onnxruntime-win-x64-gpu-1.22.0.zip` → copy every `*.dll` from its `lib\`
   folder into `ort\onnxruntime-win-x64-gpu-1.22.0\lib\`.
2. From PyPI: `nvidia-cuda-runtime-cu12` (12.9.79), `nvidia-cublas-cu12`
   (12.9.2.10), `nvidia-cufft-cu12` (11.4.1.4) and `nvidia-cudnn-cu12`
   (9.9.0.52), each the `win_amd64` wheel. A wheel is a zip file: extract
   `cudart64_12.dll` from the first, `cublas64_12.dll` and `cublasLt64_12.dll`
   from the second, `cufft64_11.dll` from the third, and all `cudnn*_9.dll`
   from the last into the same `lib\` folder.

## 4. Build and run

```powershell
cargo build --release
target\release\pdf-translate.exe "C:\path\to\document.pdf"
```

or, in one step:

```powershell
cargo run --release -- "C:\path\to\document.pdf"
```

The first build downloads the crates and takes a few minutes. Build in
`--release`: a debug build runs the model an order of magnitude slower. One or
more PDFs can be given; without a file argument the documents of the last
session are reopened, and `Open…` (Ctrl+O) picks another.
`target\release\pdf-translate.exe` can also be started from Explorer — no
working directory is required, because everything it needs is found through
absolute paths.

Two things to know about the binary:

* The location of `ort\` is compiled in (`CARGO_MANIFEST_DIR`): the binary
  looks for the `ort\` folder of the checkout it was built from. If the exe or
  the `ort\` folder is moved, set `PDF_TRANSLATE_ORT_DIR` to the folder that
  contains `onnxruntime.dll` (or to any ancestor of it — the search recurses).
* Build and run notes:
  * `cargo build` can fail with `failed to remove … pdf-translate.exe` (os
    error 5) while an instance of the app is running — close it first.
  * The `LNK4098: defaultlib 'LIBCMT' conflicts with use of other libs` warning
    comes from the Rust/ONNX Runtime mixture and is harmless.

## 5. Verify

The quickest check is the command-line translator, which prints the runtime and
the device before translating one paragraph:

```powershell
target\release\pdf-translate.exe --translate document.pdf 0 0 en qwen2.5:1.5b
```

Expected:

```
onnx runtime: C:\…\pdf-translate\ort\onnxruntime-win-x64-gpu-1.22.0\lib\onnxruntime.dll
using qwen2.5:1.5b on cuda (int4)

translation: …
```

In the app the same two lines appear in the **Log** panel (the *Log* tab in the
mode bar) as soon as the first translation starts, followed by `model ready: …`.

If the first line names `C:\Windows\System32\onnxruntime.dll`, the `ort\`
folder was not found; the IR-version error will follow when the session is
created.

`on cuda` is not just a label: while a translation runs the model holds about
1.2 GB of VRAM (about 2 GB with the working buffers), which
`nvidia-smi --query-gpu=memory.used --format=csv` or Task Manager's
*Performance → GPU* shows while it works. A session that cannot register the
CUDA provider is reported as an error naming the DLL that could not be loaded,
instead of silently running on the CPU.

## 6. Models, cache and settings

* **Models** — the first translation downloads `model_q4f16.onnx` and
  `tokenizer.json` from Hugging Face into
  `%USERPROFILE%\transcribe-models\onnx\qwen2-5-1-5b-q4f16\` (about 1.2 GB).
  The UI shows `loading the model…` while this happens, so the first
  translation takes as long as the download. `PDF_TRANSLATE_MODELS` points at
  a different folder; existing models from the `transcribe` project are reused
  as they are.
* **Translations cache** — `%USERPROFILE%\.cache\pdf-translate`, one JSON file
  per document and target language. Deleting a file re-translates that
  document; deleting the folder re-translates everything.
* **Session settings** (open documents, current page) —
  `%APPDATA%\PDF Translate\data\app.ron`.

The model folder comes from `USERPROFILE` when `HOME` is not set, so a launch
from Explorer behaves like one from PowerShell.

To reset the application completely: delete the two folders above and the
`%USERPROFILE%\transcribe-models\onnx\qwen2-5-1-5b-q4f16` model folder (it is
downloaded again on the next translation).

## 7. Configuration

| variable | effect |
| --- | --- |
| `PDF_TRANSLATE_ORT_DIR` | folder containing `onnxruntime.dll` (or an ancestor of it); overrides the built-in `ort\` search |
| `PDF_TRANSLATE_MODELS` | folder holding the ONNX models (default `%USERPROFILE%\transcribe-models\onnx`) |
| `PDF_TRANSLATE_PAGE` | open the document at this page (0-based) |
| `HTTPS_PROXY`, `ALL_PROXY` | proxy for the model download (the installer's `curl.exe` honours the same variables) |

For the current PowerShell window only:

```powershell
$env:PDF_TRANSLATE_ORT_DIR = 'D:\pdf-translate-ort'
target\release\pdf-translate.exe document.pdf
```

Permanently for the user (takes effect in new terminals):

```powershell
setx PDF_TRANSLATE_ORT_DIR "D:\pdf-translate-ort"
```

ONNX Runtime is loaded once per process and its environment is process-wide,
so restart the app after changing `ort\` or these variables.

## 8. Copying an existing setup (offline)

The two large pieces can be copied from a machine where everything works:

* `ort\` (about 2.4 GB), and set `PDF_TRANSLATE_ORT_DIR` to its `lib` folder on
  the new machine (the compiled-in path only exists on the build machine);
* `%USERPROFILE%\transcribe-models\onnx\qwen2-5-1-5b-q4f16\` (about 1.2 GB), or
  point `PDF_TRANSLATE_MODELS` at wherever it was copied.

The binary itself needs the Microsoft Visual C++ 2015-2022 redistributable
(`vcruntime140.dll`). It is present on most machines and on the one the app was
built on; if it is missing on a machine the exe was copied to, install it with
`winget install Microsoft.VCRedist.2015+.x64` (or the *Microsoft Visual C++
Redistributable* download).

## 9. Update and removal

* **Update the runtime** — re-run the installer, optionally with a newer
  `-OrtVersion` and `-Force`.
* **Update the app** — `git pull`, then `cargo build --release` (the `ort\`
  folder is not touched).
* **Remove everything** — delete `ort\`, `target\`,
  `%USERPROFILE%\transcribe-models\onnx\qwen2-5-1-5b-q4f16`,
  `%USERPROFILE%\.cache\pdf-translate` and
  `%APPDATA%\PDF Translate`.

## 10. Troubleshooting

| message | cause | fix |
| --- | --- | --- |
| `Unsupported model IR version: 10, max supported IR version: 9` | an old ONNX Runtime was loaded, usually `C:\Windows\System32\onnxruntime.dll` | check the `onnx runtime:` line; install `ort\` as above, and restart the app (the runtime is loaded once per process) |
| `Failed to load … onnxruntime.dll` / `LoadLibrary … 126` | `ort\onnxruntime-win-x64-gpu-<version>\lib\onnxruntime.dll` missing, or `PDF_TRANSLATE_ORT_DIR` points somewhere else | re-run `tools\fetch-ort-windows.ps1` |
| `... onnxruntime_providers_cuda.dll ... which depends on "cufft64_11.dll" which is missing` (or `cudnn64_9.dll`, `cublasLt64_12.dll`, …) | a CUDA library is not next to `onnxruntime.dll` | re-run the installer; do not move individual DLLs |
| `CUDA error … insufficient driver version` / `cudaErrorInsufficientDriver` | driver too old for CUDA 12.x | update the NVIDIA driver |
| `Failed to allocate memory for requested buffer` / `CUDA out of memory` | the model does not fit in VRAM | close other GPU applications; switch to `qwen2.5:1.5b`; a 3B model wants more than 4 GB |
| `model failed: … DefaultLogger …` | the ONNX Runtime environment was released | restart the app |
| translations are very slow but the log says `on cuda` | an older build, or the CUDA provider failed to register | check the `onnx runtime:` line names `ort\…`; re-run the installer so `cufft64_11.dll` is present; current builds report the failure instead of falling back |
| `failed to remove … pdf-translate.exe (os error 5)` while building, or `cannot write to …\lib` from the installer | the app is running and holds the DLLs | close it and retry |
| `requires rustc 1.92` (from `rust-version`) | toolchain too old | `rustup update stable` |
| `link.exe not found` / `LINK : fatal error LNK1181` | MSVC build tools missing | install the *Desktop development with C++* workload in Visual Studio Build Tools |
| the model is downloaded again into the current folder | launched without `HOME` on an older build | update; `USERPROFILE` is used as a fallback |
| first translation seems to hang | the model is downloading (1.2 GB) | watch the stage chip / **Log** panel; it starts translating once the download finishes |
| the model download fails or stalls (TLS, proxy, firewall) | the machine cannot reach `huggingface.co` | set `HTTPS_PROXY` / `ALL_PROXY`, or copy the model folder from another machine (see *Copying an existing setup*) |
| German text in the `--translate` output looks garbled | the console code page is not UTF-8 | run `chcp 65001` in that window, or use Windows Terminal / PowerShell 7 |
| `use of undeclared crate or module` / other build errors on a fresh clone | stale `Cargo.lock` or a different toolchain | `cargo clean`, `rustup update stable`, `cargo build --release` |
