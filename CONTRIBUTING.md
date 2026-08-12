# Guía de contribución

Gracias por tu interés en contribuir a AI Voice InterConnector. Este documento describe el
flujo de desarrollo, los estándares del proyecto y cómo proponer cambios.

## Tabla de contenidos

- [Requisitos](#requisitos)
- [Configuración del entorno de desarrollo](#configuración-del-entorno-de-desarrollo)
- [Tests](#tests)
  - [Cobertura](#cobertura)
  - [Smoke-tests de instaladores](#smoke-tests-de-instaladores)
- [Dependencias y lockfile](#dependencias-y-lockfile)
- [Compilación de binarios](#compilación-de-binarios)
- [Estilo y convenciones](#estilo-y-convenciones)
- [Flujo de Pull Request](#flujo-de-pull-request)
- [Reporte de problemas](#reporte-de-problemas)

## Requisitos

- **Python 3.13** (el proyecto fija esta versión en el build y en CI).
- Git.
- En Linux, las cabeceras de ALSA para el audio: `libasound2-dev`.

## Configuración del entorno de desarrollo

```bash
git clone https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector.git
cd AI-Voice-InterConnector

# Instalar dependencias de desarrollo (límites >=; ver nota más abajo)
pip install -r requirements.txt
pip install pytest

# Ejecutar el CLI desde el código fuente
python bin/ai-voice-interconnector version
python bin/ai-voice-interconnector doctor
```

> Los **builds y el CI** no instalan desde `requirements.txt`, sino desde
> `requirements-lock.txt` (lock universal con hashes) con `--require-hashes`, para
> garantizar reproducibilidad. `requirements.txt` son los límites de desarrollo.
>
> **Entorno idéntico al de CI** (reproduceibilidad total): si quieres que tu
> entorno local tenga exactamente las mismas versiones que en el pipeline,
> instala desde el lock en vez de desde los límites:
>
> ```bash
> python -m venv .venv
> .venv\Scripts\activate          # Linux/macOS: source .venv/bin/activate
> pip install -r requirements-lock.txt --require-hashes
> pip install pytest
> ```
>
> Esto verifica la integridad de cada paquete contra los hashes del lock. Si
> editaste `pyproject.toml` y el lock está desactualizado, el comando fallará
> con un error claro — regenera el lock primero (ver «Dependencias y lockfile»
> más abajo).

## Tests

La suite usa `pytest`. Antes de abrir un PR, ejecútala completa y asegúrate de que
pasa:

```bash
pytest tests/ -v
```

- Añade tests para todo comportamiento nuevo o corregido.
- La suite se ejecuta en CI en **Linux**, **Windows** y **macOS** nativos; evita
  supuestos específicos de un SO (rutas, permisos, señales) o márcalos con un
  `skip` justificado.
- En Windows, los tests de symlink (`TestSetupLinuxPath`, en `tests/test_cli.py`)
  requieren permiso para crear symlinks: activa el **Modo de programador**
  (Configuración → Sistema → Para programadores) o ejecuta pytest en una consola
  elevada; sin ello esos tests se saltan con `skip` y la cobertura local se reduce
  (en CI corren completos en Linux/macOS).
- Verificación rápida de sintaxis: `python -m compileall src/`.

### Cobertura

La cobertura es **opt-in**: no forma parte de `pytest tests/ -v` (no depende de
`pytest-cov`). El job `coverage` de CI la mide y aplica un gate por módulo sobre los
módulos de contrato (`cli.py`, `daemon/*`, `model_cache.py`, `voices.py`, `paths.py`).
Para reproducirlo en local:

```bash
pip install pytest-cov
pytest tests/ --cov --cov-report=json --cov-report=term-missing
python scripts/check_coverage.py coverage.json
```

Los pisos por módulo viven en `scripts/check_coverage.py::MODULE_FLOORS` (fuente
única). Si tu cambio baja deliberadamente la cobertura de un módulo de contrato,
ajusta el piso correspondiente en el mismo PR con justificación en el mensaje de
commit; si el gate falla por una regresión no intencional, sube la cobertura en vez
de bajar el piso.

### Smoke-tests de instaladores

Además de la suite pytest, los instaladores de una línea tienen smoke-tests
propios en `tests/installer/`, que corren **en CI, no en pytest**:

- `install-linux.bats` — `install-linux.sh` (Linux), con [bats-core](https://github.com/bats-core/bats-core).
- `install-macos.bats` — `install-macos.sh` (macOS), también con bats.
- `install-windows.tests.ps1` — `install-windows.ps1` (Windows), con **Pester v5**
  (`Invoke-Pester tests/installer/install-windows.tests.ps1` en PowerShell).

Si modificas un instalador, actualiza su smoke-test en el mismo cambio; los tres
jobs (`test-installer-linux`/`-windows`/`-macos`) son puerta de los builds en CI.

## Dependencias y lockfile

La fuente de verdad de las dependencias de runtime es `pyproject.toml`. Tras
modificarla, **regenera el lockfile** de forma deliberada y revisa el diff:

```bash
pip install uv
uv pip compile --universal --generate-hashes --python-version 3.13 \
    pyproject.toml -o requirements-lock.txt
```

Si cambian las dependencias empaquetadas, actualiza también
`THIRD-PARTY-LICENSES.md` (ver la sección «Regeneración» de ese archivo).

## Compilación de binarios

Ver [docs/BUILD.md](docs/BUILD.md) para el detalle por plataforma. Resumen:

```bash
npm run build-windows          # o python scripts/build_windows.py
python scripts/build_linux.py --arch x86_64
python scripts/build_macos.py --arch arm64
```

## Estilo y convenciones

- **Idioma**: el código, los comentarios, los mensajes de commit y la documentación
  se escriben en **español**, con ortografía correcta (acentos y signos incluidos).
- **Comentarios**: explican el *porqué*, no el *qué*; sigue la densidad y el estilo
  del código circundante.
- **Commits**: mensajes descriptivos en español, con prefijo de tipo cuando aplique
  (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `build:`), en imperativo.

## Flujo de Pull Request

1. Crea una rama a partir de `main`.
2. Implementa el cambio con sus tests y la actualización documental correspondiente
   (código, CI y docs deben quedar sincronizados en el mismo cambio).
3. Verifica que `pytest tests/ -v` pasa en tu máquina.
4. Abre el PR describiendo el problema, la solución y cómo verificarla.
5. Enlaza el Issue relacionado si existe.

## Reporte de problemas

- **Bugs y solicitudes de función**: abre un
  [Issue](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues).
- **Vulnerabilidades de seguridad**: sigue [SECURITY.md](SECURITY.md) (no las
  reportes en un Issue público).
