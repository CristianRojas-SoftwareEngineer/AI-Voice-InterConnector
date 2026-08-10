# Licencias de terceros (Third-Party Licenses)

TTS Sidecar se distribuye bajo **GPL-3.0-or-later** (ver `LICENSE`). El binario
autocontenido generado con PyInstaller empaqueta software de terceros bajo sus propias
licencias. Este documento reúne los avisos de copyright y las licencias correspondientes,
cuya preservación exigen dichas licencias al redistribuir el software.

Este inventario se **regenera desde `requirements-lock.txt`** (el lock universal con
hashes, fuente de verdad del build) con `pip-licenses`. La columna «Familia» es una
normalización para agrupar; la columna «Licencia (metadato)» es el dato declarado por
cada paquete y prevalece en caso de duda.

> Nota sobre PyInstaller: el proceso de empaquetado elimina los archivos de licencia
> originales de cada dependencia. Este documento los restituye. Los textos íntegros de cada
> licencia están disponibles en los enlaces canónicos indicados.

---

## Modelos de voz (no empaquetados)

Los **pesos del modelo** no se empaquetan en el binario: se descargan a la caché de
HuggingFace del usuario mediante `tts-sidecar setup`. Se listan por completitud.

| Modelo | Licencia (verificada en HuggingFace) | Fuente |
|--------|--------------------------------------|--------|
| `ResembleAI/Chatterbox-Multilingual-es-mx-latam` (language pack es-mx-latam) | **MIT** | <https://huggingface.co/ResembleAI/Chatterbox-Multilingual-es-mx-latam> |
| `ResembleAI/chatterbox` (modelo base; fuente de `ve.safetensors`) | **MIT** | <https://huggingface.co/ResembleAI/chatterbox> |
| `Helsinki-NLP/opus-mt-es-en` (traducción es→en, opt-in) | **CC-BY-4.0** | <https://huggingface.co/Helsinki-NLP/opus-mt-es-en> |
| `Helsinki-NLP/opus-mt-en-es` (traducción en→es, opt-in) | **CC-BY-4.0** | <https://huggingface.co/Helsinki-NLP/opus-mt-en-es> |

Los repositorios de Chatterbox declaran licencia **MIT** en sus metadatos (verificado el
2026-07-03). El modelo base incluye además la nota de que su salida lleva un watermark
neural (PerthNet) y un descargo de uso responsable («Don't use this model to do bad
things»). TTS Sidecar **desactiva ese watermark** en el motor; ver la sección «Uso ético y
responsable» en `README.md`/`USAGE.md` para las obligaciones que ello traslada al usuario.

### Atribución: opus-mt (`Helsinki-NLP/opus-mt-es-en` / `opus-mt-en-es`)

El par de modelos de traducción **opus-mt** usado por el subsistema opcional de
traducción cross-lingual (`translate`, y `speech say|synthesize --source-language`/
`--target-language`) es obra del proyecto **OPUS-MT** del grupo **Language Technology
Research Group de la Universidad de Helsinki** (Helsinki-NLP), distribuido bajo
**CC-BY-4.0** (Creative Commons Attribution 4.0). Los pesos se descargan a demanda
mediante `tts-sidecar setup --language en/all` y se convierten localmente a formato
CTranslate2; no se empaquetan en el binario. Fichas del modelo: mismos enlaces que la
tabla anterior. Texto de la licencia: <https://creativecommons.org/licenses/by/4.0/>.

### Atribución: PerthNet (`resemble-perth`)

El watermarker neural **PerthNet** es obra de **Resemble AI** y se distribuye como el
paquete Python `resemble-perth` (licencia **MIT**, © Resemble AI;
<https://pypi.org/project/resemble-perth/>), dependencia de `chatterbox-tts`. Aunque
TTS Sidecar **no ejecuta** el watermarker (el engine lo bypasea en ambos modos), el
paquete **sí se redistribuye** dentro del binario autocontenido, por lo que su aviso de
copyright y su licencia se conservan aquí y en la tabla de dependencias empaquetadas
(fila `resemble-perth`).

---

## Familias de licencias permisivas

La mayoría de las dependencias empaquetadas usan licencias permisivas compatibles con
GPLv3: **MIT**, **BSD (2/3-Clause)**, **Apache-2.0**, **ISC** y **PSF-2.0**.

- **MIT** — permiso de uso, copia, modificación y distribución conservando el aviso de
  copyright y la nota de permiso. Texto: <https://opensource.org/license/mit>.
- **BSD (2/3-Clause)** — redistribución conservando el aviso de copyright, la lista de
  condiciones y el descargo de garantía. Texto 3-Clause:
  <https://opensource.org/license/bsd-3-clause>.
- **Apache-2.0** — conserva avisos de copyright, el texto de la licencia y el archivo
  `NOTICE` cuando el trabajo lo incluya (p. ej. los proyectos de Hugging Face:
  `transformers`, `diffusers`, `safetensors`, `tokenizers`, `huggingface-hub`). Texto:
  <https://www.apache.org/licenses/LICENSE-2.0>. NOTICE de Hugging Face:
  <https://github.com/huggingface>.
- **ISC** — funcionalmente equivalente a MIT/BSD-2. Texto:
  <https://opensource.org/license/isc-license-txt>.
- **PSF-2.0** — Python Software Foundation License, permisiva y compatible con GPLv3.
  Texto: <https://docs.python.org/3/license.html>.

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

## Componentes con obligaciones adicionales (copyleft y bibliotecas nativas)

Estas dependencias no son permisivas puras; todas son **compatibles con GPLv3**, pero
imponen obligaciones que se documentan aquí.

### MPL-2.0 (Mozilla Public License)

Copyleft por archivo, con cláusula explícita de compatibilidad con GPL. Obligación:
poner a disposición el código fuente de los archivos cubiertos si se modifican. Texto:
<https://www.mozilla.org/MPL/2.0/>.

| Componente | Licencia (metadato) |
|-----------|---------------------|
| `certifi` | Mozilla Public License 2.0 (MPL 2.0) |
| `orjson` | MPL-2.0 AND (Apache-2.0 OR MIT) |
| `tqdm` | MPL-2.0 AND MIT |

### LGPL-2.1-or-later + libsndfile

Copyleft débil: permite el enlace desde software bajo otra licencia siempre que el
usuario pueda sustituir/re-enlazar la biblioteca LGPL. Compatible con GPLv3. Texto:
<https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html>.

| Componente | Licencia | Nota |
|-----------|----------|------|
| `soxr` | LGPL-2.1-or-later | Binding Python del resampler SoX. |
| **libsndfile** | LGPL-2.1-or-later | **No es un paquete pip**: la biblioteca nativa C se empaqueta dentro de los wheels de `soundfile` (que en sí es BSD). Se declara aquí por la obligación LGPL de la biblioteca enlazada. Fuente: <https://github.com/libsndfile/libsndfile>. |

### GPL-3.0-or-later

| Componente | Licencia | Nota |
|-----------|----------|------|
| `pykakasi` | GNU GPL v3 or later (GPLv3+) | Transliteración de japonés (dependencia transitiva del stack multilingüe). Misma licencia que el proyecto; sin conflicto. Fuente: <https://codeberg.org/miurahr/pykakasi>. |

### Componentes propietarios redistribuibles: NVIDIA CUDA (solo Linux x64)

El build de **Linux x64** empaqueta los runtimes de CUDA que `torch` arrastra en esa
plataforma (`nvidia-*`, `cuda-*`). **No** son open source: se redistribuyen bajo el
**NVIDIA CUDA Toolkit EULA**, que permite la redistribución de los componentes de runtime.
No están presentes en los builds de Windows (CUDA embebido en el wheel de torch) ni de
macOS arm64 (CPU/MPS, sin CUDA). Texto del EULA:
<https://docs.nvidia.com/cuda/eula/index.html>.

El lockfile universal `requirements-lock.txt` *incluye* estos paquetes
`nvidia-*`/`cuda-*` porque es el grafo que resuelve `torch` para
`linux`/`x86_64` y que habilita la aceleración NVIDIA en esa plataforma. Su
presencia en el lock es **deliberada y está correctamente acotada**: el AppImage
de Linux x86_64 se construye desde `requirements-lock-linux-cpu.txt` (lock
CPU-only) y por tanto no los empaqueta; el build nativo Linux x64 sí los
incluye, pero su redistribución está permitida por el EULA citado arriba y se
documenta en esta sección. Un auditor no debe interpretar el `nvidia-*` en el
lock universal como un problema de licencia del ejecutable.

---

## Herramienta de empaquetado: PyInstaller

TTS Sidecar se compila con **PyInstaller**, distribuido bajo **GPL 2.0 con una excepción**
que permite redistribuir los ejecutables generados bajo cualquier licencia, siempre que no
se modifique el *bootloader*. TTS Sidecar usa el bootloader estándar sin modificar.
Texto y excepción: <https://pyinstaller.org/en/stable/license.html>. (PyInstaller es una
herramienta de build; no forma parte del contenido redistribuido más allá del bootloader.)

---

## Inventario completo del lockfile

Generado desde `requirements-lock.txt` (159 paquetes de runtime, directos y transitivos).
Los paquetes `nvidia-*`/`cuda-*` (31) **no forman parte de ningún artefacto distribuido**:
el build de Linux x64 usa el lock CPU-only (`requirements-lock-linux-cpu.txt`, sin
`nvidia-*`) y en los demás builds esos paquetes están excluidos por marcador de
plataforma. Solo aplican a una instalación desde código fuente con el lock universal
en Linux x86_64.

Resumen por familia: MIT 54 · BSD 37 · NVIDIA (propietaria) 31 · Apache-2.0 23 ·
PSF-2.0 6 · MPL-2.0 3 · ISC 2 · LGPL-2.1+ 1 · GPLv3+ 1 · sin metadato 1.

| Paquete | Versión | Licencia (metadato) | Familia |
|---------|---------|---------------------|--------|
| `accelerate` | 1.14.0 | Apache Software License | Apache-2.0 |
| `aiofiles` | 24.1.0 | Apache Software License | Apache-2.0 |
| `annotated-doc` | 0.0.4 | MIT | MIT |
| `annotated-types` | 0.7.0 | MIT License | MIT |
| `antlr4-python3-runtime` | 4.9.3 | BSD | BSD |
| `anyio` | 4.14.1 | MIT | MIT |
| `audioop-lts` | 0.2.2 | PSF-2.0 | PSF-2.0 |
| `audioread` | 3.1.0 | MIT | MIT |
| `brotli` | 1.2.0 | MIT | MIT |
| `catalogue` | 2.0.10 | MIT License | MIT |
| `certifi` | 2026.6.17 | Mozilla Public License 2.0 (MPL 2.0) | MPL-2.0 |
| `cffi` | 2.0.0 | MIT | MIT |
| `cfgv` | 3.5.0 | MIT | MIT |
| `charset-normalizer` | 3.4.7 | MIT | MIT |
| `chatterbox-tts` | 0.1.7 | MIT License | MIT |
| `click` | 8.4.2 | BSD-3-Clause | BSD |
| `colorama` | 0.4.6 | BSD License | BSD |
| `comtypes` | 1.4.16 | MIT | MIT |
| `conformer` | 0.3.2 | MIT License | MIT |
| `cuda-bindings` | 13.3.1 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `cuda-pathfinder` | 1.5.6 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `cuda-toolkit` | 13.0.2 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `decorator` | 5.3.1 | BSD-2-Clause | BSD |
| `deprecated` | 1.3.1 | MIT License | MIT |
| `diffusers` | 0.29.0 | Apache Software License | Apache-2.0 |
| `distlib` | 0.4.3 | Python Software Foundation License | PSF-2.0 |
| `einops` | 0.8.2 | MIT License | MIT |
| `fastapi` | 0.139.0 | MIT | MIT |
| `ffmpy` | 1.0.0 | MIT | MIT |
| `filelock` | 3.29.5 | MIT | MIT |
| `fsspec` | 2026.6.0 | BSD-3-Clause | BSD |
| `gradio` | 6.8.0 | Apache-2.0 | Apache-2.0 |
| `gradio-client` | 2.2.0 | Apache-2.0 | Apache-2.0 |
| `groovy` | 0.1.2 | MIT License | MIT |
| `h11` | 0.16.0 | MIT License | MIT |
| `hf-xet` | 1.5.1 | Apache-2.0 | Apache-2.0 |
| `httpcore` | 1.0.9 | BSD-3-Clause | BSD |
| `httptools` | 0.8.0 | MIT | MIT |
| `httpx` | 0.28.1 | BSD License | BSD |
| `huggingface-hub` | 1.22.0 | Apache Software License | Apache-2.0 |
| `identify` | 2.6.19 | MIT | MIT |
| `idna` | 3.18 | BSD-3-Clause | BSD |
| `importlib-metadata` | 9.0.0 | Apache-2.0 | Apache-2.0 |
| `jaconv` | 0.5.0 | MIT License | MIT |
| `jinja2` | 3.1.6 | BSD License | BSD |
| `joblib` | 1.5.3 | BSD-3-Clause | BSD |
| `lazy-loader` | 0.5 | BSD-3-Clause | BSD |
| `librosa` | 0.11.0 | ISC License (ISCL) | ISC |
| `llvmlite` | 0.36.0 | BSD-2-Clause AND Apache-2.0 WITH LLVM-exception | Apache-2.0 |
| `markdown-it-py` | 4.2.0 | MIT License | MIT |
| `markupsafe` | 3.0.3 | BSD-3-Clause | BSD |
| `mdurl` | 0.1.2 | MIT License | MIT |
| `ml-dtypes` | 0.5.4 | Apache-2.0 | Apache-2.0 |
| `mpmath` | 1.3.0 | BSD License | BSD |
| `msgpack` | 1.2.1 | Apache-2.0 | Apache-2.0 |
| `narwhals` | 2.23.0 | MIT | MIT |
| `networkx` | 3.6.1 | BSD-3-Clause | BSD |
| `nodeenv` | 1.10.0 | BSD License | BSD |
| `numba` | 0.53.1 | BSD License | BSD |
| `numpy` | 2.5.0 | BSD-3-Clause AND 0BSD AND MIT AND Zlib AND CC0-1.0 | BSD |
| `nvidia-cublas` | 13.1.1.3 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cublas-cu12` | 12.4.5.8 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cuda-cupti` | 13.0.85 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cuda-cupti-cu12` | 12.4.127 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cuda-nvrtc` | 13.0.88 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cuda-nvrtc-cu12` | 12.4.127 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cuda-runtime` | 13.0.96 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cuda-runtime-cu12` | 12.4.127 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cudnn-cu12` | 9.1.0.70 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cudnn-cu13` | 9.20.0.48 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cufft` | 12.0.0.61 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cufft-cu12` | 11.2.1.3 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cufile` | 1.15.1.6 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-curand` | 10.4.0.35 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-curand-cu12` | 10.3.5.147 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cusolver` | 12.0.4.66 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cusolver-cu12` | 11.6.1.9 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cusparse` | 12.6.3.3 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cusparse-cu12` | 12.3.1.170 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cusparselt-cu12` | 0.6.2 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-cusparselt-cu13` | 0.8.1 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nccl-cu12` | 2.21.5 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nccl-cu13` | 2.29.7 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nvjitlink` | 13.0.88 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nvjitlink-cu12` | 12.4.127 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nvshmem-cu13` | 3.4.5 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nvtx` | 13.0.85 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `nvidia-nvtx-cu12` | 12.4.127 | NVIDIA CUDA EULA | NVIDIA (propietaria, redistribuible) |
| `omegaconf` | 2.3.1 | BSD License | BSD |
| `onnx` | 1.22.0 | Apache-2.0 | Apache-2.0 |
| `orjson` | 3.11.9 | MPL-2.0 AND (Apache-2.0 OR MIT) | MPL-2.0 |
| `packaging` | 26.2 | Apache-2.0 OR BSD-2-Clause | Apache-2.0 |
| `pandas` | 3.0.3 | BSD License | BSD |
| `peft` | 0.20.0 | Apache Software License | Apache-2.0 |
| `pillow` | 12.3.0 | MIT-CMU | MIT |
| `platformdirs` | 4.10.0 | MIT | MIT |
| `pooch` | 1.9.0 | BSD-3-Clause | BSD |
| `pre-commit` | 4.6.0 | MIT | MIT |
| `protobuf` | 7.35.1 | 3-Clause BSD License | BSD |
| `pysbd` | 0.3.4 | MIT | MIT |
| `psutil` | 7.2.2 | BSD-3-Clause | BSD |
| `pycaw` | 20251023 | UNKNOWN (MIT según el repo del proyecto) | Sin metadato (ver nota) |
| `pycparser` | 3.0 | BSD-3-Clause | BSD |
| `pydantic` | 2.13.4 | MIT | MIT |
| `pydantic-core` | 2.46.4 | MIT | MIT |
| `pydub` | 0.25.1 | MIT License | MIT |
| `pygments` | 2.20.0 | BSD-2-Clause | BSD |
| `pykakasi` | 2.3.0 | GNU General Public License v3 or later (GPLv3+) | GPL-3.0-or-later |
| `pyloudnorm` | 0.2.0 | MIT | MIT |
| `python-dateutil` | 2.9.0.post0 | Apache Software License; BSD License | Apache-2.0 |
| `python-discovery` | 1.4.3 | MIT License | MIT |
| `python-dotenv` | 1.2.2 | BSD-3-Clause | BSD |
| `python-multipart` | 0.0.32 | Apache-2.0 | Apache-2.0 |
| `pytz` | 2026.2 | MIT License | MIT |
| `pyyaml` | 6.0.3 | MIT License | MIT |
| `regex` | 2026.6.28 | Apache-2.0 AND CNRI-Python | Apache-2.0 |
| `requests` | 2.34.2 | Apache Software License | Apache-2.0 |
| `resemble-perth` | 1.0.1 | MIT License | MIT |
| `rich` | 15.0.0 | MIT License | MIT |
| `s3tokenizer` | 0.3.0 | Apache2.0 | Apache-2.0 |
| `safehttpx` | 0.1.7 | MIT License | MIT |
| `safetensors` | 0.5.3 | Apache Software License | Apache-2.0 |
| `scikit-learn` | 1.9.0 | BSD-3-Clause | BSD |
| `scipy` | 1.18.0 | BSD License | BSD |
| `semantic-version` | 2.10.0 | BSD License | BSD |
| `setuptools` | 81.0.0 | MIT | MIT |
| `shellingham` | 1.5.4 | ISC License (ISCL) | ISC |
| `six` | 1.17.0 | MIT License | MIT |
| `sounddevice` | 0.5.5 | MIT | MIT |
| `soundfile` | 0.14.0 | BSD License (empaqueta libsndfile, LGPL-2.1+) | BSD |
| `soxr` | 1.1.0 | LGPL-2.1-or-later | LGPL-2.1-or-later |
| `spacy-pkuseg` | 1.0.1 | MIT License | MIT |
| `srsly` | 2.5.3 | MIT License | MIT |
| `standard-aifc` | 3.13.0 | Python Software Foundation License | PSF-2.0 |
| `standard-chunk` | 3.13.0 | Python Software Foundation License | PSF-2.0 |
| `standard-sunau` | 3.13.0 | Python Software Foundation License | PSF-2.0 |
| `starlette` | 0.52.1 | BSD-3-Clause | BSD |
| `sympy` | 1.14.0 | BSD License | BSD |
| `threadpoolctl` | 3.6.0 | BSD License | BSD |
| `tokenizers` | 0.22.2 | Apache Software License | Apache-2.0 |
| `tomlkit` | 0.13.3 | MIT License | MIT |
| `torch` | 2.12.1 | BSD License | BSD |
| `torchaudio` | 2.11.0 | BSD License | BSD |
| `tqdm` | 4.68.3 | MPL-2.0 AND MIT | MPL-2.0 |
| `transformers` | 5.2.0 | Apache 2.0 License | Apache-2.0 |
| `triton` | 3.7.1 | MIT | MIT |
| `typer` | 0.26.8 | MIT | MIT |
| `typer-slim` | 0.24.0 | MIT | MIT |
| `typing-extensions` | 4.16.0 | PSF-2.0 | PSF-2.0 |
| `typing-inspection` | 0.4.2 | MIT | MIT |
| `tzdata` | 2026.2 | Apache-2.0 | Apache-2.0 |
| `urllib3` | 2.7.0 | MIT | MIT |
| `uvicorn` | 0.49.0 | BSD-3-Clause | BSD |
| `uvloop` | 0.22.1 | MIT | MIT |
| `virtualenv` | 21.5.1 | MIT | MIT |
| `watchfiles` | 1.2.0 | MIT | MIT |
| `websockets` | 16.0 | BSD-3-Clause | BSD |
| `wrapt` | 2.2.2 | BSD-2-Clause | BSD |
| `zipp` | 4.1.0 | MIT | MIT |

> `pycaw` (solo Windows) declara metadato de licencia `UNKNOWN` en su distribución, pero
> su repositorio (<https://github.com/AndreMiras/pycaw>) publica el proyecto bajo **MIT**.

---

## Regeneración

Este inventario se regenera de forma **deliberada** tras actualizar `requirements-lock.txt`:

```bash
pip install pip-licenses
# Sobre un entorno instalado desde el lock:
pip-licenses --format=markdown --with-authors --with-urls
```

Revisar el diff resultante para auditar altas/bajas de dependencias y cambios de licencia
antes de commitear.
