# Licencias de terceros (Third-Party Licenses)

AI Voice InterConnector se distribuye bajo **GPL-3.0-or-later** (ver `LICENSE`). El binario
autocontenido Rust empaqueta software de terceros bajo sus propias
licencias. Este documento reúne los avisos de copyright y las licencias correspondientes,
cuya preservación exigen dichas licencias al redistribuir el software.

Este inventario se **regenera desde `Cargo.lock`** (lockfile Rust, fuente de verdad del build) con
`cargo metadata` / `cargo-license`. La columna «Familia» es una
normalización para agrupar; la columna «Licencia (metadato)» es el dato declarado por
cada crate y prevalece en caso de duda.

> Nota: los pesos de modelos no se empaquetan en el binario; van a `~/.cache/huggingface/hub` vía `setup`.

---

## Modelos de voz (no empaquetados)

Los **pesos del modelo** no se empaquetan en el binario: se descargan a la caché de
HuggingFace del usuario mediante `ai-voice-interconnector setup`. Se listan por completitud.

| Modelo | Licencia (verificada en HuggingFace) | Fuente |
|--------|--------------------------------------|--------|
| `qwen3-tts-0.6b` / `qwen3-tts-0.6b-base` (motor Qwen3-TTS, Base opt-in) | **MIT / Apache-2.0** | <https://github.com/QwenLM/Qwen3-TTS> |
| `Helsinki-NLP/opus-mt-es-en` / `opus-mt-en-es` (traducción) | **CC-BY-4.0** | <https://huggingface.co/Helsinki-NLP/opus-mt-es-en> |
| `istupakov/parakeet-tdt-0.6b-v3-onnx` (Parakeet TDT 0.6B v3 int8, ONNX) | **CC-BY-4.0** | <https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx> |

---

## Familias de licencias permisivas

La mayoría de los crates empaquetados usan licencias permisivas compatibles con
GPLv3: **MIT**, **Apache-2.0**, **BSD**, **ISC**, **Unlicense**.

- **MIT** — permiso de uso, copia, modificación y distribución conservando el aviso de
  copyright y la nota de permiso. Texto: <https://opensource.org/license/mit>.
- **Apache-2.0** — conserva avisos de copyright, el texto de la licencia y el archivo
  `NOTICE`. Texto: <https://www.apache.org/licenses/LICENSE-2.0>.
- **BSD (2/3-Clause)** — redistribución conservando el aviso de copyright. Texto:
  <https://opensource.org/license/bsd-3-clause>.
- **ISC** — funcionalmente equivalente a MIT/BSD-2. Texto:
  <https://opensource.org/license/isc-license-txt>.
- **Unlicense** — dominio público. Texto: <https://unlicense.org>.

```
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## Componentes con obligaciones adicionales

Solo crates permisivos y el propio proyecto (GPL-3.0-or-later) están presentes en el
binario Rust. No hay componentes MPL/LGPL/GPL adicionales más allá del proyecto.
Los pesos de modelos y OpenBLAS (BSD-3-Clause, GCC Runtime Exception) se documentan
en `docs/BUILD.md` §9.

---

## Inventario completo del lockfile

Generado desde `Cargo.lock` (455 crates únicos, directos y transitivos).
Resumen por familia: Apache-2.0 17, BSD 4, CDLA-Permissive-2.0 1, GPL-3.0-or-later 9, ISC 3, MIT 393, MPL-2.0 2, Unicode-3.0 18, Zlib 4.

| Paquete | Versión | Licencia (metadato) | Familia |
|---------|---------|---------------------|--------|
| `ahash` | 0.8.12 | MIT OR Apache-2.0 | MIT |
| `aho-corasick` | 1.1.5 | Unlicense OR MIT | MIT |
| `ai-voice-interconnector` | 0.18.26 | GPL-3.0-or-later | GPL-3.0-or-later |
| `alsa` | 0.9.1 | Apache-2.0/MIT | MIT |
| `alsa-sys` | 0.3.1 | MIT | MIT |
| `android_system_properties` | 0.1.6 | MIT OR Apache-2.0 | MIT |
| `anstream` | 1.0.0 | MIT OR Apache-2.0 | MIT |
| `anstyle` | 1.0.14 | MIT OR Apache-2.0 | MIT |
| `anstyle-parse` | 1.0.0 | MIT OR Apache-2.0 | MIT |
| `anstyle-query` | 1.1.5 | MIT OR Apache-2.0 | MIT |
| `anstyle-wincon` | 3.0.11 | MIT OR Apache-2.0 | MIT |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | MIT |
| `approx` | 0.5.1 | Apache-2.0 | Apache-2.0 |
| `arrayvec` | 0.7.8 | MIT OR Apache-2.0 | MIT |
| `async-trait` | 0.1.92 | MIT OR Apache-2.0 | MIT |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT | MIT |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT | MIT |
| `avi-audio` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-config` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-core` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-daemon` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-store` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-stt` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-translation` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `avi-tts` | 0.1.0 | GPL-3.0-or-later | GPL-3.0-or-later |
| `aws-lc-rs` | 1.18.0 | ISC AND (Apache-2.0 OR ISC) | Apache-2.0 |
| `aws-lc-sys` | 0.44.0 | ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | MIT |
| `axum` | 0.7.9 | MIT | MIT |
| `axum-core` | 0.4.5 | MIT | MIT |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | MIT |
| `bindgen` | 0.72.1 | BSD-3-Clause | BSD |
| `bitflags` | 2.13.1 | MIT OR Apache-2.0 | MIT |
| `blake3` | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | Apache-2.0 |
| `block-buffer` | 0.12.1 | MIT OR Apache-2.0 | MIT |
| `block2` | 0.6.2 | MIT | MIT |
| `bon` | 3.10.0 | MIT OR Apache-2.0 | MIT |
| `bon-macros` | 3.10.0 | MIT OR Apache-2.0 | MIT |
| `bstr` | 1.13.1 | MIT OR Apache-2.0 | MIT |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 | MIT |
| `bytemuck` | 1.25.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `bytes` | 1.12.1 | MIT | MIT |
| `castaway` | 0.2.4 | MIT OR Apache-2.0 | MIT |
| `cc` | 1.4.2 | MIT OR Apache-2.0 | MIT |
| `cesu8` | 1.1.0 | Apache-2.0/MIT | MIT |
| `cexpr` | 0.6.0 | Apache-2.0/MIT | MIT |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | MIT |
| `cfg_aliases` | 0.2.2 | MIT | MIT |
| `chacha20` | 0.10.1 | MIT OR Apache-2.0 | MIT |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | MIT |
| `clang-sys` | 1.9.1 | Apache-2.0 | Apache-2.0 |
| `clap` | 4.6.6 | MIT OR Apache-2.0 | MIT |
| `clap_builder` | 4.6.6 | MIT OR Apache-2.0 | MIT |
| `clap_derive` | 4.6.4 | MIT OR Apache-2.0 | MIT |
| `clap_lex` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `cmake` | 0.1.58 | MIT OR Apache-2.0 | MIT |
| `codespan-reporting` | 0.13.1 | MIT OR Apache-2.0 | MIT |
| `colorchoice` | 1.0.5 | MIT OR Apache-2.0 | MIT |
| `colored` | 3.1.1 | MPL-2.0 | MPL-2.0 |
| `combine` | 4.6.7 | MIT | MIT |
| `compact_str` | 0.9.1 | MIT OR Apache-2.0 | MIT |
| `console` | 0.16.4 | MIT | MIT |
| `const-oid` | 0.10.2 | Apache-2.0 OR MIT | MIT |
| `const-str` | 1.1.0 | MIT | MIT |
| `const_panic` | 0.2.17 | Zlib | Zlib |
| `constant_time_eq` | 0.4.2 | CC0-1.0 OR MIT-0 OR Apache-2.0 | MIT |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 | MIT |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 | MIT |
| `coreaudio-rs` | 0.11.3 | MIT/Apache-2.0 | MIT |
| `coreaudio-sys` | 0.2.18 | MIT | MIT |
| `countio` | 0.3.0 | MIT | MIT |
| `cpal` | 0.15.3 | Apache-2.0 | Apache-2.0 |
| `cpufeatures` | 0.3.0 | MIT OR Apache-2.0 | MIT |
| `crc32fast` | 1.5.1 | MIT OR Apache-2.0 | MIT |
| `crossbeam-channel` | 0.5.16 | MIT OR Apache-2.0 | MIT |
| `crossbeam-deque` | 0.8.7 | MIT OR Apache-2.0 | MIT |
| `crossbeam-epoch` | 0.9.20 | MIT OR Apache-2.0 | MIT |
| `crossbeam-utils` | 0.8.22 | MIT OR Apache-2.0 | MIT |
| `crypto-common` | 0.2.2 | MIT OR Apache-2.0 | MIT |
| `ct2rs` | 0.10.0 | MIT OR Apache-2.0 | MIT |
| `ctor` | 1.0.13 | Apache-2.0 OR MIT | MIT |
| `ctrlc` | 3.5.2 | MIT/Apache-2.0 | MIT |
| `cxx` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxx-build` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxxbridge-cmd` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxxbridge-flags` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxxbridge-macro` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `darling` | 0.24.1 | MIT | MIT |
| `darling_core` | 0.24.1 | MIT | MIT |
| `darling_macro` | 0.24.1 | MIT | MIT |
| `dary_heap` | 0.3.9 | MIT OR Apache-2.0 | MIT |
| `dasp_sample` | 0.11.0 | MIT OR Apache-2.0 | MIT |
| `deranged` | 0.5.8 | MIT OR Apache-2.0 | MIT |
| `derive_builder` | 0.20.2 | MIT OR Apache-2.0 | MIT |
| `derive_builder_core` | 0.20.2 | MIT OR Apache-2.0 | MIT |
| `derive_builder_macro` | 0.20.2 | MIT OR Apache-2.0 | MIT |
| `digest` | 0.11.3 | MIT OR Apache-2.0 | MIT |
| `directories` | 5.0.1 | MIT OR Apache-2.0 | MIT |
| `dirs` | 6.0.0 | MIT OR Apache-2.0 | MIT |
| `dirs-sys` | 0.5.0 | MIT OR Apache-2.0 | MIT |
| `dispatch2` | 0.3.1 | Zlib OR Apache-2.0 OR MIT | MIT |
| `displaydoc` | 0.2.7 | MIT OR Apache-2.0 | MIT |
| `dunce` | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | MIT |
| `either` | 1.17.0 | MIT OR Apache-2.0 | MIT |
| `encode_unicode` | 1.0.0 | Apache-2.0 OR MIT | MIT |
| `encoding_rs` | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause | MIT |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT | MIT |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | MIT |
| `esaxx-rs` | 0.1.10 | MIT OR Apache-2.0 | MIT |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | MIT |
| `find-msvc-tools` | 0.1.10 | MIT OR Apache-2.0 | MIT |
| `fnv` | 1.0.7 | Apache-2.0 / MIT | MIT |
| `foldhash` | 0.2.0 | MIT OR Apache-2.0 | MIT |
| `form_urlencoded` | 1.2.2 | MIT OR Apache-2.0 | MIT |
| `fs_extra` | 1.3.0 | MIT | MIT |
| `futures` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-channel` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-core` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-executor` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-io` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-macro` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-sink` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-task` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `gearhash` | 0.1.3 | MIT OR Apache-2.0 | MIT |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | MIT |
| `git-version` | 0.3.9 | BSD-2-Clause | BSD |
| `git-version-macro` | 0.3.9 | BSD-2-Clause | BSD |
| `glob` | 0.3.4 | MIT OR Apache-2.0 | MIT |
| `globset` | 0.4.20 | Unlicense OR MIT | MIT |
| `h2` | 0.4.19 | MIT | MIT |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 | MIT |
| `heapify` | 0.2.0 | MIT OR Apache-2.0 | MIT |
| `heck` | 0.5.0 | MIT OR Apache-2.0 | MIT |
| `hermit-abi` | 0.5.2 | MIT OR Apache-2.0 | MIT |
| `hf-hub` | 1.0.0 | Apache-2.0 | Apache-2.0 |
| `hf-xet` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `hound` | 3.5.1 | Apache-2.0 | Apache-2.0 |
| `http` | 1.5.0 | MIT OR Apache-2.0 | MIT |
| `http-body` | 1.1.0 | MIT | MIT |
| `http-body-util` | 0.1.4 | MIT | MIT |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 | MIT |
| `httpdate` | 1.0.3 | MIT OR Apache-2.0 | MIT |
| `humantime` | 2.4.0 | MIT OR Apache-2.0 | MIT |
| `hybrid-array` | 0.4.14 | MIT OR Apache-2.0 | MIT |
| `hyper` | 1.11.0 | MIT | MIT |
| `hyper-rustls` | 0.27.9 | Apache-2.0 OR ISC OR MIT | MIT |
| `hyper-util` | 0.1.20 | MIT | MIT |
| `iana-time-zone` | 0.1.65 | MIT OR Apache-2.0 | MIT |
| `iana-time-zone-haiku` | 0.1.2 | MIT OR Apache-2.0 | MIT |
| `icu_collections` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_locale_core` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_normalizer` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_normalizer_data` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_properties` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_properties_data` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_provider` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `ident_case` | 1.0.1 | MIT/Apache-2.0 | MIT |
| `idna` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `idna_adapter` | 1.2.2 | Apache-2.0 OR MIT | MIT |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT | MIT |
| `indicatif` | 0.18.6 | MIT | MIT |
| `ipnet` | 2.12.1 | MIT OR Apache-2.0 | MIT |
| `is_terminal_polyfill` | 1.70.2 | MIT OR Apache-2.0 | MIT |
| `itertools` | 0.14.0 | MIT OR Apache-2.0 | MIT |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | MIT |
| `jni` | 0.22.4 | MIT OR Apache-2.0 | MIT |
| `jni-macros` | 0.22.4 | MIT OR Apache-2.0 | MIT |
| `jni-sys` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `jni-sys-macros` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `jobserver` | 0.1.35 | MIT OR Apache-2.0 | MIT |
| `js-sys` | 0.3.104 | MIT OR Apache-2.0 | MIT |
| `konst` | 0.4.3 | Zlib | Zlib |
| `konst_proc_macros` | 0.4.1 | Zlib | Zlib |
| `lazy_static` | 1.5.0 | MIT OR Apache-2.0 | MIT |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | MIT |
| `libloading` | 0.8.9 | ISC | ISC |
| `libredox` | 0.1.19 | MIT | MIT |
| `link-cplusplus` | 1.0.12 | MIT OR Apache-2.0 | MIT |
| `link-section` | 0.19.3 | Apache-2.0 OR MIT | MIT |
| `linktime-proc-macro` | 0.2.3 | Apache-2.0 OR MIT | MIT |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `litemap` | 0.8.3 | Unicode-3.0 | Unicode-3.0 |
| `lock_api` | 0.4.14 | MIT OR Apache-2.0 | MIT |
| `log` | 0.4.33 | MIT OR Apache-2.0 | MIT |
| `lru-slab` | 0.1.2 | MIT OR Apache-2.0 OR Zlib | MIT |
| `lz4_flex` | 0.13.1 | MIT | MIT |
| `mach2` | 0.4.3 | BSD-2-Clause OR MIT OR Apache-2.0 | MIT |
| `macro_rules_attribute` | 0.2.3 | MIT OR Apache-2.0 | MIT |
| `macro_rules_attribute-proc_macro` | 0.2.3 | MIT OR Apache-2.0 | MIT |
| `matchers` | 0.2.0 | MIT | MIT |
| `matchit` | 0.7.3 | MIT AND BSD-3-Clause | MIT |
| `matrixmultiply` | 0.3.11 | MIT/Apache-2.0 | MIT |
| `memchr` | 2.8.3 | Unlicense OR MIT | MIT |
| `mime` | 0.3.17 | MIT OR Apache-2.0 | MIT |
| `mime_guess` | 2.0.5 | MIT | MIT |
| `minimal-lexical` | 0.2.1 | MIT/Apache-2.0 | MIT |
| `mio` | 1.2.2 | MIT | MIT |
| `monostate` | 0.1.18 | MIT OR Apache-2.0 | MIT |
| `monostate-impl` | 0.1.18 | MIT OR Apache-2.0 | MIT |
| `more-asserts` | 0.3.1 | Unlicense OR MIT OR Apache-2.0 OR CC0-1.0 | MIT |
| `ndarray` | 0.17.2 | MIT OR Apache-2.0 | MIT |
| `ndk` | 0.8.0 | MIT OR Apache-2.0 | MIT |
| `ndk-context` | 0.1.1 | MIT OR Apache-2.0 | MIT |
| `ndk-sys` | 0.5.0+25.2.9519653 | MIT OR Apache-2.0 | MIT |
| `nix` | 0.31.3 | MIT | MIT |
| `nom` | 7.1.3 | MIT | MIT |
| `ntapi` | 0.4.3 | Apache-2.0 OR MIT | MIT |
| `nu-ansi-term` | 0.50.3 | MIT | MIT |
| `num-complex` | 0.4.6 | MIT OR Apache-2.0 | MIT |
| `num-conv` | 0.2.2 | MIT OR Apache-2.0 | MIT |
| `num-derive` | 0.4.2 | MIT OR Apache-2.0 | MIT |
| `num-integer` | 0.1.46 | MIT OR Apache-2.0 | MIT |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 | MIT |
| `num_cpus` | 1.17.0 | MIT OR Apache-2.0 | MIT |
| `num_enum` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | MIT |
| `num_enum_derive` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | MIT |
| `number_prefix` | 0.4.0 | MIT | MIT |
| `objc2` | 0.6.4 | MIT | MIT |
| `objc2-core-foundation` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-encode` | 4.1.0 | MIT | MIT |
| `objc2-io-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-system-configuration` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `oboe` | 0.6.1 | Apache-2.0 | Apache-2.0 |
| `oboe-sys` | 0.6.1 | Apache-2.0 | Apache-2.0 |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | MIT |
| `once_cell_polyfill` | 1.70.2 | MIT OR Apache-2.0 | MIT |
| `onednn-src` | 0.1.15 | MIT OR Apache-2.0 | MIT |
| `oneshot` | 0.1.13 | MIT OR Apache-2.0 | MIT |
| `onig` | 6.5.3 | MIT OR Apache-2.0 | MIT |
| `onig_sys` | 69.9.3 | MIT OR Apache-2.0 | MIT |
| `openssl-probe` | 0.2.1 | MIT OR Apache-2.0 | MIT |
| `option-ext` | 0.2.0 | MPL-2.0 | MPL-2.0 |
| `ort` | 2.0.0-rc.13 | MIT OR Apache-2.0 | MIT |
| `ort-sys` | 2.0.0-rc.13 | MIT OR Apache-2.0 | MIT |
| `os_str_bytes` | 6.6.1 | MIT OR Apache-2.0 | MIT |
| `parking_lot` | 0.12.5 | MIT OR Apache-2.0 | MIT |
| `parking_lot_core` | 0.9.12 | MIT OR Apache-2.0 | MIT |
| `paste` | 1.0.15 | MIT OR Apache-2.0 | MIT |
| `pastey` | 0.2.3 | MIT OR Apache-2.0 | MIT |
| `pathdiff` | 0.2.3 | MIT/Apache-2.0 | MIT |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | MIT |
| `pin-project` | 1.1.13 | Apache-2.0 OR MIT | MIT |
| `pin-project-internal` | 1.1.13 | Apache-2.0 OR MIT | MIT |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT | MIT |
| `pkg-config` | 0.3.33 | MIT OR Apache-2.0 | MIT |
| `portable-atomic` | 1.15.0 | Apache-2.0 OR MIT | MIT |
| `portable-atomic-util` | 0.2.7 | Apache-2.0 OR MIT | MIT |
| `potential_utf` | 0.1.6 | Unicode-3.0 | Unicode-3.0 |
| `powerfmt` | 0.2.0 | MIT OR Apache-2.0 | MIT |
| `ppv-lite86` | 0.2.21 | MIT OR Apache-2.0 | MIT |
| `prettyplease` | 0.3.0 | MIT OR Apache-2.0 | MIT |
| `primal-check` | 0.3.4 | MIT OR Apache-2.0 | MIT |
| `proc-macro-crate` | 3.5.0 | MIT OR Apache-2.0 | MIT |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | MIT |
| `prost` | 0.14.4 | MIT OR Apache-2.0 | MIT |
| `prost-derive` | 0.14.4 | MIT OR Apache-2.0 | MIT |
| `quinn` | 0.11.11 | MIT OR Apache-2.0 | MIT |
| `quinn-proto` | 0.11.17 | MIT OR Apache-2.0 | MIT |
| `quinn-udp` | 0.5.15 | MIT OR Apache-2.0 | MIT |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | MIT |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | MIT |
| `rand` | 0.10.2 | MIT OR Apache-2.0 | MIT |
| `rand_chacha` | 0.9.0 | MIT OR Apache-2.0 | MIT |
| `rand_core` | 0.10.1 | MIT OR Apache-2.0 | MIT |
| `rand_pcg` | 0.10.2 | MIT OR Apache-2.0 | MIT |
| `rawpointer` | 0.2.1 | MIT/Apache-2.0 | MIT |
| `rayon` | 1.12.0 | MIT OR Apache-2.0 | MIT |
| `rayon-cond` | 0.4.0 | MIT OR Apache-2.0 | MIT |
| `rayon-core` | 1.13.0 | MIT OR Apache-2.0 | MIT |
| `realfft` | 3.5.0 | MIT | MIT |
| `redb` | 3.1.3 | MIT OR Apache-2.0 | MIT |
| `redox_syscall` | 0.5.18 | MIT | MIT |
| `redox_users` | 0.5.2 | MIT | MIT |
| `regex` | 1.13.1 | MIT OR Apache-2.0 | MIT |
| `regex-automata` | 0.4.18 | MIT OR Apache-2.0 | MIT |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 | MIT |
| `reqwest` | 0.13.4 | MIT OR Apache-2.0 | MIT |
| `reqwest-middleware` | 0.5.2 | MIT OR Apache-2.0 | MIT |
| `ring` | 0.17.14 | Apache-2.0 AND ISC | Apache-2.0 |
| `rubato` | 0.16.2 | MIT | MIT |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT | MIT |
| `rustc_version` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `rustfft` | 6.4.1 | MIT OR Apache-2.0 | MIT |
| `rustix` | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `rustls` | 0.23.43 | Apache-2.0 OR ISC OR MIT | MIT |
| `rustls-native-certs` | 0.8.4 | Apache-2.0 OR ISC OR MIT | MIT |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | MIT |
| `rustls-platform-verifier` | 0.7.0 | MIT OR Apache-2.0 | MIT |
| `rustls-platform-verifier-android` | 0.1.1 | MIT OR Apache-2.0 | MIT |
| `rustls-webpki` | 0.103.15 | ISC | ISC |
| `rustversion` | 1.0.23 | MIT OR Apache-2.0 | MIT |
| `ryu` | 1.0.23 | Apache-2.0 OR BSL-1.0 | Apache-2.0 |
| `safe-transmute` | 0.11.3 | MIT | MIT |
| `same-file` | 1.0.6 | Unlicense/MIT | MIT |
| `schannel` | 0.1.29 | MIT | MIT |
| `scopeguard` | 1.2.0 | MIT OR Apache-2.0 | MIT |
| `scratch` | 1.0.9 | MIT OR Apache-2.0 | MIT |
| `security-framework` | 3.7.0 | MIT OR Apache-2.0 | MIT |
| `security-framework-sys` | 2.17.0 | MIT OR Apache-2.0 | MIT |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | MIT |
| `sentencepiece` | 0.13.2 | MIT OR Apache-2.0 | MIT |
| `sentencepiece-sys` | 0.13.2 | MIT OR Apache-2.0 | MIT |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | MIT |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | MIT |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | MIT |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 | MIT |
| `serde_path_to_error` | 0.1.20 | MIT OR Apache-2.0 | MIT |
| `serde_repr` | 0.1.21 | MIT OR Apache-2.0 | MIT |
| `serde_spanned` | 0.6.9 | MIT OR Apache-2.0 | MIT |
| `serde_urlencoded` | 0.7.1 | MIT/Apache-2.0 | MIT |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | MIT |
| `sharded-slab` | 0.1.7 | MIT | MIT |
| `shellexpand` | 3.1.2 | MIT/Apache-2.0 | MIT |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | MIT |
| `signal-hook-registry` | 1.4.8 | MIT OR Apache-2.0 | MIT |
| `simd_cesu8` | 1.2.0 | Apache-2.0 OR MIT | MIT |
| `simdutf8` | 0.1.5 | MIT OR Apache-2.0 | MIT |
| `slab` | 0.4.12 | MIT | MIT |
| `smallvec` | 1.15.2 | MIT OR Apache-2.0 | MIT |
| `socket2` | 0.6.5 | MIT OR Apache-2.0 | MIT |
| `spm_precompiled` | 0.1.4 | MIT OR Apache-2.0 | MIT |
| `stable_deref_trait` | 1.2.1 | MIT OR Apache-2.0 | MIT |
| `static_assertions` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `statrs` | 0.18.0 | MIT | MIT |
| `strength_reduce` | 0.2.4 | MIT OR Apache-2.0 | MIT |
| `strsim` | 0.11.1 | MIT | MIT |
| `subtle` | 2.6.1 | BSD-3-Clause | BSD |
| `symlink` | 0.1.0 | MIT/Apache-2.0 | MIT |
| `syn` | 3.0.3 | MIT OR Apache-2.0 | MIT |
| `sync_wrapper` | 1.0.2 | Apache-2.0 | Apache-2.0 |
| `synstructure` | 0.13.2 | MIT | MIT |
| `sysinfo` | 0.38.4 | MIT | MIT |
| `system-configuration` | 0.7.0 | MIT OR Apache-2.0 | MIT |
| `system-configuration-sys` | 0.6.0 | MIT OR Apache-2.0 | MIT |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | MIT |
| `termcolor` | 1.4.1 | MIT OR Apache-2.0 | MIT |
| `thiserror` | 2.0.20 | MIT OR Apache-2.0 | MIT |
| `thiserror-impl` | 2.0.20 | MIT OR Apache-2.0 | MIT |
| `thread_local` | 1.1.10 | MIT OR Apache-2.0 | MIT |
| `time` | 0.3.55 | MIT OR Apache-2.0 | MIT |
| `time-core` | 0.1.9 | MIT OR Apache-2.0 | MIT |
| `time-macros` | 0.2.32 | MIT OR Apache-2.0 | MIT |
| `tinystr` | 0.8.4 | Unicode-3.0 | Unicode-3.0 |
| `tinyvec` | 1.12.0 | Zlib OR Apache-2.0 OR MIT | MIT |
| `tinyvec_macros` | 0.1.1 | MIT OR Apache-2.0 OR Zlib | MIT |
| `tokenizers` | 0.22.2 | MIT OR Apache-2.0 | MIT |
| `tokio` | 1.53.1 | MIT | MIT |
| `tokio-macros` | 2.7.2 | MIT | MIT |
| `tokio-retry` | 0.3.2 | MIT | MIT |
| `tokio-rustls` | 0.26.4 | MIT OR Apache-2.0 | MIT |
| `tokio-stream` | 0.1.19 | MIT | MIT |
| `tokio-util` | 0.7.19 | MIT | MIT |
| `tokio_with_wasm` | 0.8.8 | MIT | MIT |
| `tokio_with_wasm_proc` | 0.8.8 | MIT | MIT |
| `toml` | 0.8.23 | MIT OR Apache-2.0 | MIT |
| `toml_datetime` | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | MIT |
| `toml_edit` | 0.25.13+spec-1.1.0 | MIT OR Apache-2.0 | MIT |
| `toml_parser` | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | MIT |
| `toml_write` | 0.1.2 | MIT OR Apache-2.0 | MIT |
| `tower` | 0.5.3 | MIT | MIT |
| `tower-http` | 0.6.11 | MIT | MIT |
| `tower-layer` | 0.3.3 | MIT | MIT |
| `tower-service` | 0.3.3 | MIT | MIT |
| `tracing` | 0.1.44 | MIT | MIT |
| `tracing-appender` | 0.2.5 | MIT | MIT |
| `tracing-attributes` | 0.1.31 | MIT | MIT |
| `tracing-core` | 0.1.36 | MIT | MIT |
| `tracing-log` | 0.2.0 | MIT | MIT |
| `tracing-serde` | 0.2.0 | MIT | MIT |
| `tracing-subscriber` | 0.3.23 | MIT | MIT |
| `transpose` | 0.2.3 | MIT OR Apache-2.0 | MIT |
| `try-lock` | 0.2.5 | MIT | MIT |
| `twox-hash` | 2.1.3 | MIT | MIT |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | MIT |
| `typewit` | 1.15.2 | Zlib | Zlib |
| `unicase` | 2.9.0 | MIT OR Apache-2.0 | MIT |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | MIT |
| `unicode-normalization-alignments` | 0.1.12 | MIT OR Apache-2.0 | MIT |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | MIT |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 | MIT |
| `unicode_categories` | 0.1.1 | MIT OR Apache-2.0 | MIT |
| `unit-prefix` | 0.5.2 | MIT OR Apache-2.0 | MIT |
| `untrusted` | 0.9.0 | ISC | ISC |
| `url` | 2.5.8 | MIT OR Apache-2.0 | MIT |
| `urlencoding` | 2.1.3 | MIT | MIT |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT | MIT |
| `utf8parse` | 0.2.2 | Apache-2.0 OR MIT | MIT |
| `uuid` | 1.25.0 | Apache-2.0 OR MIT | MIT |
| `valuable` | 0.1.1 | MIT | MIT |
| `version_check` | 0.9.5 | MIT OR Apache-2.0 | MIT |
| `walkdir` | 2.5.0 | Unlicense/MIT | MIT |
| `want` | 0.3.1 | MIT | MIT |
| `wasi` | 0.14.7+wasi-0.2.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `wasip2` | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `wasite` | 1.0.2 | Apache-2.0 OR BSL-1.0 OR MIT | MIT |
| `wasm-bindgen` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-futures` | 0.4.77 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-macro` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-macro-support` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-shared` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-streams` | 0.5.0 | MIT OR Apache-2.0 | MIT |
| `web-sys` | 0.3.104 | MIT OR Apache-2.0 | MIT |
| `web-time` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `webpki-root-certs` | 1.0.9 | CDLA-Permissive-2.0 | CDLA-Permissive-2.0 |
| `whoami` | 2.1.3 | Apache-2.0 OR BSL-1.0 OR MIT | MIT |
| `winapi` | 0.3.9 | MIT/Apache-2.0 | MIT |
| `winapi-i686-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | MIT |
| `winapi-util` | 0.1.11 | Unlicense OR MIT | MIT |
| `winapi-x86_64-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | MIT |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | MIT |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | MIT |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | MIT |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | MIT |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 | MIT |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 | MIT |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | MIT |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | MIT |
| `windows-registry` | 0.6.1 | MIT OR Apache-2.0 | MIT |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | MIT |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | MIT |
| `windows-targets` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | MIT |
| `windows_aarch64_gnullvm` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_aarch64_msvc` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_i686_gnu` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_i686_gnullvm` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_i686_msvc` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_gnu` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_gnullvm` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_msvc` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `winnow` | 1.0.4 | MIT | MIT |
| `winreg` | 0.52.0 | MIT | MIT |
| `wit-bindgen` | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `writeable` | 0.6.4 | Unicode-3.0 | Unicode-3.0 |
| `xet-client` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xet-core-structures` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xet-data` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xet-runtime` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xtask` | 0.1.0 | MIT OR Apache-2.0 | MIT |
| `yoke` | 0.8.3 | Unicode-3.0 | Unicode-3.0 |
| `yoke-derive` | 0.8.2 | Unicode-3.0 | Unicode-3.0 |
| `zerocopy` | 0.8.56 | MIT OR Apache-2.0 | MIT |
| `zerocopy-derive` | 0.8.56 | MIT OR Apache-2.0 | MIT |
| `zerofrom` | 0.1.8 | Unicode-3.0 | Unicode-3.0 |
| `zerofrom-derive` | 0.1.7 | Unicode-3.0 | Unicode-3.0 |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT | MIT |
| `zerotrie` | 0.2.5 | Unicode-3.0 | Unicode-3.0 |
| `zerovec` | 0.11.7 | Unicode-3.0 | Unicode-3.0 |
| `zerovec-derive` | 0.11.4 | Unicode-3.0 | Unicode-3.0 |
| `zmij` | 1.0.23 | MIT | MIT |

> `ai-voice-interconnector` y `avi-*` son crates locales del proyecto (GPL-3.0-or-later).

---

## Regeneración

Este inventario se regenera de forma **deliberada** tras actualizar `Cargo.lock`:

```bash
cargo metadata --format-version 1 | python scripts/check_third_party_licenses.py
python scripts/check_third_party_licenses.py  # verifica sincronía
```

Revisar el diff resultante para auditar altas/bajas de dependencias y cambios de licencia
antes de commitear.