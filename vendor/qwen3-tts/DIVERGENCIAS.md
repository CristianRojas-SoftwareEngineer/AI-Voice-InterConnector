# Divergencias del snapshot respecto al motor original

Este directorio es un snapshot del motor Qwen3-TTS, sin submódulo ni historial upstream. Cualquier actualización del motor debe volver a aplicar las divergencias de este archivo; si no, se pierde la corrección que cada una implementa.

## Procedencia

- Repositorio de origen: <https://github.com/gabriele-mastrapasqua/qwen3-tts>.
- Snapshot incorporado el 2026-08-12, sin historial.
- El commit upstream exacto del que se tomó no se registró.

## Divergencias

### Bind del servidor con `--host` y loopback por defecto

- **Archivos**: `main.c`, `qwen_tts_server.c`, `qwen_tts_server.h`.
- **Qué cambia**: el flag `--host <ipv4>` (por defecto `127.0.0.1`) se valida al parsear los argumentos con `inet_pton` y llega a `setup_listen_socket`, que hace `bind` exactamente a esa dirección. Una dirección inválida sale con código 1 antes de cargar el modelo. Los banners imprimen la dirección real. Las funciones `qwen_tts_serve*` reciben `host` como segundo parámetro.
- **Por qué**: el original enlazaba en `INADDR_ANY` y el servidor no tiene autenticación, así que cualquier equipo de la red local podía sintetizar y enumerar voces. El valor por defecto de loopback y el rechazo de direcciones inválidas son de seguridad: nunca debe volverse en silencio a todas las interfaces.
- **Si se pierde**: el motor vuelve a escuchar en todas las interfaces y queda expuesto a la red local, aunque el lanzador pase `--host` (el motor lo ignoraría o fallaría).

### `WSAStartup` para `--serve` en Windows

- **Archivos**: `main.c` (inicio de `main`).
- **Qué cambia**: inicializa winsock antes de cualquier llamada a `socket()` o `bind()`.
- **Por qué**: sin él, la primera llamada winsock devuelve `WSANOTINITIALISED` y el servidor aborta.
- **Si se pierde**: `--serve` no arranca en Windows.

### Shims POSIX para Windows

- **Archivos**: `third_party/ingot/mingw_shim/` (`unistd.h`, `sys/mman.h`, `sys/socket.h`, `arpa/inet.h`, `netinet/in.h`) y `posix_shim_win.h`.
- **Qué cambia**: definen sobre Win32 los símbolos POSIX que el motor asume (`mmap`, `pread`, `posix_memalign`, `setenv`, sockets). El `Makefile` usa `third_party/ingot/mingw_shim/`; `posix_shim_win.h` es una adición local que el `Makefile` no referencia.
- **Por qué**: el motor asume Linux y hay que compilarlo con MinGW/UCRT64.
- **Si se pierde**: el motor no compila en Windows.

### Ramas MinGW/UCRT64 y ARM del `Makefile`

- **Archivos**: `Makefile`.
- **Qué cambia**: rama MinGW/UCRT64 (shims, OpenBLAS de `/ucrt64`, enlazado `-static` con `-lws2_32`) y rama ARM aarch64 que honra `SIMD` (`native`, etc.) con NEON como línea base portable. Deja anotado que las variantes deterministas de flags (`-ffp-contract=off`, con y sin `-ffast-math`) se evaluaron y se descartaron porque degradan el WER de `dub` corto; se mantiene `-ffast-math`.
- **Por qué**: obtener un `qwen_tts.exe` autocontenido en Windows y binarios portables en ARM.
- **Si se pierde**: no compila en Windows, o el binario ARM no es portable a CPUs más antiguas.

### Bloqueo del tokenizer y EOS de textos cortos

- **Archivos**: `qwen_tts_tokenizer.c` (validación UTF-8 y guarda de progreso), `qwen_tts.c` (refuerzo de EOS para textos cortos).
- **Qué cambia**: el tokenizer valida UTF-8 y aborta si no progresa; la generación refuerza el EOS en textos cortos.
- **Por qué**: ciertos textos colgaban el tokenizer y los textos cortos degeneraban en audio largo con ruido.
- **Si se pierde**: reaparecen el bloqueo del tokenizer y la degeneración de textos cortos.

### Poda de tooling y documentación del original

- **Archivos**: se eliminaron `blog/`, `docs/` (parcial), `tests/`, `tools/` y `training/`.
- **Qué cambia**: solo se conserva lo necesario para compilar y ejecutar el motor.
- **Por qué**: reducir el tamaño del snapshot.
- **Si se pierde**: no aplica, pero quedan rotos los enlaces del `README.md` a `docs/server.md` y similares, y los objetivos `test-serve-repro` y `bench-server` del `Makefile`.

### Corrección de la afirmación sobre AVX2 en `CLAUDE.md`

- **Archivos**: `CLAUDE.md`.
- **Qué cambia**: la descripción de `qwen_tts_kernels.c` indica que los bucles calientes sí contienen AVX2 (verificado el 2026-08-24).
- **Por qué**: el texto original afirmaba lo contrario.
- **Si se pierde**: la guía vuelve a describir mal los kernels; no afecta al binario.
