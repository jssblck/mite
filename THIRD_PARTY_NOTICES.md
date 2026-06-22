# Third-Party Notices

Mite is licensed under the GNU Affero General Public License v3.0. Third-party
software, model files, dictionaries, and data used with Mite remain under their
own license terms. This file summarizes the main third-party materials that a
developer or redistributor should know about.

This notice is informational and is not a substitute for the upstream license
texts.

## Rust Dependencies

Rust crate dependencies are resolved through `Cargo.lock`. Their license
metadata is recorded in the crates published on crates.io or in their source
repositories. At the time this notice was added, the dependency graph was
predominantly permissive (`MIT`, `Apache-2.0`, `BSD`, `ISC`, `Unicode-3.0`, and
similar terms). Some crates expose multiple license choices; for example, `r-efi`
may be used under permissive alternatives rather than LGPL terms.

To refresh dependency license metadata locally:

```powershell
cargo metadata --format-version 1 --locked
```

## Desktop App Dependencies

The desktop app under `app\` is built with Tauri v2 and bundles a web frontend.
Its dependencies remain under their own terms:

- Tauri and its plugins (Tauri Programme within The Commons Conservancy):
  dual-licensed Apache-2.0 or MIT.
- Microsoft Edge WebView2 runtime: the app renders through the system WebView2
  control, which is governed by Microsoft's WebView2 distribution terms and is
  preinstalled on current Windows. It is not redistributed in this repository.
- Frontend libraries (React, Vite, and related tooling): predominantly MIT.
  JavaScript dependency license metadata is recorded in `app\bun.lock` and the
  installed packages.
- Fonts (Hanken Grotesk, Geist Mono, Noto Sans JP) are loaded at runtime from
  Google Fonts and are licensed under the SIL Open Font License; they are not
  committed to this repository.

The app does not bundle the OCR models or GPU runtime; it downloads them at
runtime from the sources described below and in `model-manifest.json`.

## OCR Models And Character Dictionary

Mite's default OCR assets are downloaded by
`scripts\bootstrap-dev.ps1 -ModelsOnly` from RapidAI/RapidOCR's published
PP-OCRv5 ONNX model set. The repository does not commit these runtime model
files; they are installed into `models\`.

- Upstream project: RapidAI/RapidOCR
- Homepage: https://github.com/RapidAI/RapidOCR
- Model source: https://www.modelscope.cn/models/RapidAI/RapidOCR
- Default manifest reference: `model-manifest.json`
- License recorded in manifest: Apache-2.0

The default downloaded assets are:

- `pp-ocrv5-mobile-det.onnx`
- `pp-ocrv5-mobile-rec.onnx`
- `pp-ocrv5-dict.txt`

Optional PP-OCRv5 server models can be downloaded with
`scripts\bootstrap-dev.ps1 -ModelsOnly -IncludeServerModels` as described in
`docs\models.md`; those files are also runtime artifacts under `models\`.

## ONNX Runtime And NVIDIA Runtime Libraries

Mite uses ONNX Runtime through the Rust `ort` crate. ONNX Runtime provider
libraries and NVIDIA TensorRT/CUDA/cuDNN runtime DLLs are not committed to this
repository. `scripts\bootstrap-dev.ps1 -GpuRuntimeOnly` stages redistributable
runtime DLLs into `.gpu-runtime\bin` for local use and build-time copying.

Review the upstream NVIDIA and ONNX Runtime license terms before redistributing
any staged binary runtime artifacts.

## JMdict / EDRDG Dictionary Data

Mite downloads the English JMdict-derived lexicon through
`scriptin/jmdict-simplified` into `models\jmdict-eng.json`.

- Data: JMdict/EDICT dictionary data
- Copyright: Electronic Dictionary Research and Development Group (EDRDG)
- Packaged via: https://github.com/scriptin/jmdict-simplified
- License recorded in manifest: CC-BY-SA-4.0
- Attribution recorded in manifest: `model-manifest.json`

Redistribution of JMdict-derived data must comply with the EDRDG/JMdict and
CC-BY-SA terms.

## Lindera And Embedded IPADIC

Mite uses Lindera with the embedded IPADIC feature for Japanese morphological
analysis. Lindera and its embedded dictionary data are third-party materials
distributed under their upstream license terms through the Rust dependency graph.

- Lindera: https://github.com/lindera/lindera
- Crate feature used by Mite: `embed-ipadic`

## JPDB Frequency List

Mite uses the JPDB frequency list to improve Japanese segmentation decisions.
The data is downloaded into `models\jpdb-freq\`.

- Source project: https://github.com/MarvNC/jpdb-freq-list
- Archive URL recorded in: `model-manifest.json`
- Attribution recorded in manifest: JPDB frequency list by Marv

Review the upstream project before redistributing the downloaded frequency data.

## Evaluation Captures

Real-image evaluation captures under `eval\` are regression fixtures for Mite's
OCR and lookup behavior. They are private data, stored separately from this
source repository and separate from the runtime model and dictionary downloads.
Do not publish, mirror, or redistribute the eval submodule without an explicit
review of the captured images, labels, and upstream content in those captures.

## Repo-Local Agent Skills

The repository vendors `leonardomso/rust-skills` under
`.agents\skills\rust-skills` for agent guidance during development.

- Upstream project: https://github.com/leonardomso/rust-skills
- Vendored commit: `89910e8585331dabbecd400ae132b4070ecf24af`
- License: MIT
