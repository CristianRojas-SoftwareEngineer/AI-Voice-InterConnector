#!/usr/bin/env python3
"""
speaker_similarity.py — gate C2 (timbre) para el cierre de la Fase 5 del CLI.

Mide cuánto conserva el locutor un WAV sintetizado respecto de un audio de
referencia, comparando el speaker-embedding (x-vector) de ambos por cosine.

A diferencia de compare_audio.py (que compara espectros mel bit-a-bit, útil solo
en muestreo greedy/determinista), esta métrica es robusta al muestreo estocástico
(temp>0): dos renders de la misma voz con distinto seed tienen mel-corr bajo pero
speaker-similarity alto, porque el timbre se conserva aunque la prosodia varíe.

El x-vector se extrae con el propio motor Qwen3-TTS (su speaker encoder, 1024-d),
que es el mismo espacio en el que el TTS condiciona la voz — de ahí que mida el
timbre "tal como el modelo lo ve" (criterio C2: "en el mismo build"):

    qwen_tts(.exe) -d <base_model> --ref-audio <wav> --xvector-only --save-voice <bin>

El .bin resultante son 1024 float32 little-endian (4096 bytes).

Centrado (por defecto): los x-vectors crudos de este encoder viven en un cono de
cosine alto (mismo-locutor ~0.98 vs otro-locutor ~0.94: margen ~0.04, mal
discriminador). Restar una media de cohorte de locutores diversos (whitening lite)
separa las clases: mismo-locutor ~0.88 vs otro-locutor <=0.50 (margen ~0.37). La
media se calibró (F2) con los 9 presets CustomVoice y se sirve en el asset
`speaker_cohort_mean.npy`, junto a este archivo. Umbral por defecto 0.70 (a mitad,
con margen ~0.17 a cada lado).

Uso:
    speaker_similarity.py <referencia> <salida> [--min-sim 0.70] [--label txt]
                          [--center PATH|--no-center] [--engine PATH] [--model PATH]

Cada entrada puede ser un .wav (se extrae el x-vector con el motor) o un .bin ya
precomputado (1024 float32 LE), lo que permite reutilizar x-vectors entre pares.

Salida: PASS/FAIL con el score. Exit: 0 = PASS, 1 = FAIL (bajo umbral), 2 = error.
"""
import os
import sys
import argparse
import subprocess
import tempfile

import numpy as np

# Rutas por defecto relativas a este archivo (vendor/qwen3-tts/tests/).
_HERE = os.path.dirname(os.path.abspath(__file__))
_VENDOR = os.path.dirname(_HERE)
_DEF_ENGINE = os.path.join(_VENDOR, "qwen_tts.exe" if os.name == "nt" else "qwen_tts")
_DEF_MODEL = os.path.join(_VENDOR, "qwen3-tts-0.6b-base")  # el x-vector exige el modelo Base
_DEF_CENTER = os.path.join(_HERE, "speaker_cohort_mean.npy")  # media de cohorte (F2)

XVEC_DIM = 1024


def load_xvector(path, engine, model):
    """Devuelve el x-vector 1024-d de un .bin precomputado o de un .wav (vía motor)."""
    if path.lower().endswith(".bin"):
        return _read_bin(path)

    # Extraer el x-vector del WAV con el speaker encoder del motor.
    with tempfile.TemporaryDirectory() as td:
        out_bin = os.path.join(td, "xvec.bin")
        cmd = [engine, "-d", model, "--ref-audio", path,
               "--xvector-only", "--save-voice", out_bin]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0 or not os.path.exists(out_bin):
            sys.stderr.write(f"speaker_similarity: fallo al extraer x-vector de {path}\n")
            sys.stderr.write(proc.stderr[-2000:] + "\n")
            sys.exit(2)
        return _read_bin(out_bin)


def _read_bin(path):
    v = np.fromfile(path, dtype="<f4")
    if v.size != XVEC_DIM:
        sys.stderr.write(f"speaker_similarity: x-vector inesperado en {path} "
                         f"({v.size} floats, se esperaban {XVEC_DIM})\n")
        sys.exit(2)
    return v.astype(np.float64)


def cosine(a, b, mu=None):
    if mu is not None:
        a, b = a - mu, b - mu
    na, nb = np.linalg.norm(a), np.linalg.norm(b)
    if na == 0 or nb == 0:
        return 0.0
    return float(np.dot(a, b) / (na * nb))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("reference", help="Audio de referencia (.wav) o x-vector (.bin)")
    ap.add_argument("output", help="Audio sintetizado a evaluar (.wav) o x-vector (.bin)")
    ap.add_argument("--min-sim", type=float, default=0.70,
                    help="Umbral de cosine para PASS (0.70 calibrado con anclas en F2)")
    ap.add_argument("--center", default=_DEF_CENTER,
                    help="Media de cohorte (.npy) para centrar; por defecto el asset F2")
    ap.add_argument("--no-center", action="store_true",
                    help="Cosine crudo, sin centrar (margen ~0.04; no recomendado)")
    ap.add_argument("--label", default="")
    ap.add_argument("--engine", default=_DEF_ENGINE, help="Binario del motor qwen_tts")
    ap.add_argument("--model", default=_DEF_MODEL, help="Directorio del modelo Base")
    a = ap.parse_args()

    for p in (a.reference, a.output):
        if not os.path.exists(p):
            sys.stderr.write(f"speaker_similarity: no existe {p}\n")
            sys.exit(2)

    mu = None
    if not a.no_center:
        if not os.path.exists(a.center):
            sys.stderr.write(f"speaker_similarity: falta la media de centrado {a.center} "
                             f"(usa --no-center para cosine crudo)\n")
            sys.exit(2)
        mu = np.load(a.center).astype(np.float64)
        if mu.size != XVEC_DIM:
            sys.stderr.write(f"speaker_similarity: media de centrado inesperada "
                             f"({mu.size} floats, se esperaban {XVEC_DIM})\n")
            sys.exit(2)

    ref = load_xvector(a.reference, a.engine, a.model)
    out = load_xvector(a.output, a.engine, a.model)
    sim = cosine(ref, out, mu)

    status = "PASS" if sim >= a.min_sim else "FAIL"
    print(f"{status} {a.label}: speaker_sim={sim:.5f} (>= {a.min_sim})")
    if status == "FAIL":
        sys.stderr.write(f"  speaker-similarity too low: {sim:.5f} < {a.min_sim}\n")
        sys.exit(1)
    sys.exit(0)


if __name__ == "__main__":
    main()
