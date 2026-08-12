#!/usr/bin/env python3
"""
Genera el paquete de distribución e instalador nativo en Windows para ai-voice-interconnector (Rust).

Usage:
    python scripts/build_release_native.py [--output <output_dir>]

Example:
    python scripts/build_release_native.py --output dist
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

def build_rust_release() -> Path:
    """Compila el ejecutable nativo en modo release usando cargo."""
    print("=== Compilando binario nativo en modo release (cargo build --release) ===")
    cmd = ["cargo", "build", "--release"]
    res = subprocess.run(cmd, cwd=REPO_ROOT)
    if res.returncode != 0:
        print("Error: cargo build --release falló.", file=sys.stderr)
        sys.exit(1)

    target_exe = REPO_ROOT / "target" / "release" / "ai-voice-interconnector.exe"
    if not target_exe.exists():
        print(f"Error: No se encontró el ejecutable en {target_exe}", file=sys.stderr)
        sys.exit(1)

    print(f"Binario nativo compilado exitosamente: {target_exe}")
    return target_exe

def create_native_bundle(release_exe: Path, dist_dir: Path) -> Path:
    """Crea la estructura del paquete distribuible autocontenido."""
    bundle_dir = dist_dir / "ai-voice-interconnector-native-win-x64"
    if bundle_dir.exists():
        shutil.rmtree(bundle_dir)

    bundle_dir.mkdir(parents=True, exist_ok=True)

    # Copiar binario nativo principal
    shutil.copy2(release_exe, bundle_dir / "ai-voice-interconnector.exe")

    # Copiar documentación y licencias
    for doc in ["LICENSE", "THIRD-PARTY-LICENSES.md", "SOURCE-OFFER.md", "README.md"]:
        doc_path = REPO_ROOT / doc
        if doc_path.exists():
            shutil.copy2(doc_path, bundle_dir / doc)

    # Crear directorio para modelos y voces de fábrica
    (bundle_dir / "voices" / "default").mkdir(parents=True, exist_ok=True)
    (bundle_dir / "models").mkdir(parents=True, exist_ok=True)

    print(f"Paquete distribuible creado exitosamente en: {bundle_dir}")
    return bundle_dir

def main():
    parser = argparse.ArgumentParser(description="Build de distribución nativo en Rust para Windows.")
    parser.add_argument("--output", default=str(REPO_ROOT / "dist"), help="Directorio de salida para la distribución")
    args = parser.parse_args()

    dist_dir = Path(args.output)
    dist_dir.mkdir(parents=True, exist_ok=True)

    # 1. Compilar release
    release_exe = build_rust_release()

    # 2. Empaquetar bundle
    bundle_dir = create_native_bundle(release_exe, dist_dir)

    print("=== Proceso de empaquetado nativo finalizado con éxito ===")
    print(f"Directorio de distribución: {bundle_dir.resolve()}")

if __name__ == "__main__":
    main()
