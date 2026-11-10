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

### Vigía de la entrada estándar con `--watch-stdin`

- **Archivos**: `main.c`.
- **Qué cambia**: el flag `--watch-stdin` (sin valor) arranca, tras parsear y validar los argumentos y antes de cargar el modelo, un hilo desacoplado que lee la entrada estándar en bucle y descarta lo que llegue. Al fin de fichero (o ante un error de lectura distinto de `EINTR`) el hilo llama a `_exit(0)`. Si `pthread_create` falla, el motor escribe un error por stderr y sale con código 1. Sin el flag no cambia nada: ningún modo lee la entrada estándar.
- **Por qué**: el lanzador mantiene abierto el extremo de escritura de una tubería conectada a la entrada estándar del motor; cuando el lanzador muere de forma abrupta el sistema operativo cierra la tubería y el motor termina en lugar de quedar huérfano. Un fallo al crear el hilo no puede ignorarse, porque se perdería en silencio la garantía de que el motor muere con quien lo lanzó.
- **Si se pierde**: el motor sobrevive a la muerte abrupta de su lanzador, y el modo residente (`--serve`) sigue ocupando el puerto y la memoria indefinidamente.

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
- **Qué cambia**: rama MinGW/UCRT64 (shims, OpenBLAS de `/ucrt64`, enlazado `-static` con `-lws2_32`) y rama ARM aarch64 que honra `SIMD` (`native`, etc.) con `-march=armv8.2-a+dotprod` como línea base: el motor exige `dotprod` (ver «Exigencia de `dotprod` en Linux ARM64»). Deja anotado que las variantes deterministas de flags (`-ffp-contract=off`, con y sin `-ffast-math`) se evaluaron y se descartaron porque degradan el WER de `dub` corto; se mantiene `-ffast-math`.
- **Por qué**: obtener un `qwen_tts.exe` autocontenido en Windows y binarios ARM portables a toda CPU ARMv8.2 con `dotprod`.
- **Si se pierde**: no compila en Windows, o el binario ARM se compila sin `dotprod` y falla con `#error` o deja de ser portable a las CPUs de campo.

### Manifiesto UTF-8 incrustado en `qwen_tts.exe` (Windows)

- **Archivos**: `qwen_tts.exe.manifest`, `qwen_tts_manifest.rc` y la rama MinGW del `Makefile` (variable `WIN_RES_OBJ`, regla `qwen_tts_manifest.o` con `windres -O coff`). `main.c` no cambia.
- **Qué cambia**: el ejecutable lleva incrustado un manifiesto de aplicación con `<activeCodePage>UTF-8</activeCodePage>`. Con él, el CRT entrega `argv` en UTF-8 y las APIs ANSI (`fopen` y similares) interpretan las rutas como UTF-8.
- **Por qué**: sin manifiesto, `main(argc, argv)` recibe los argumentos en la página ANSI del sistema (cp1252). Una ruta con caracteres no ASCII en `--ref-audio` o en `--save-voice` (p. ej. `cañón/x.qvoice`) no llegaba íntegra: el archivo se creaba con el nombre corrupto o la apertura fallaba.
- **Requisito**: Windows 10 versión 1903 o posterior; en versiones anteriores el manifiesto se ignora y vuelve el comportamiento cp1252.
- **Verificación**: la cadena `activeCodePage` aparece en `qwen_tts.exe`; clonar con `--ref-audio` (24 kHz mono) y `--save-voice cañón/x.qvoice` crea el archivo en esa ruta y la cabecera `QVCE` queda válida.
- **Si se pierde**: `--ref-audio`, `--save-voice` y las rutas no ASCII se corrompen en Windows sin ningún error.

### Cancelación de la generación al desconectarse el cliente

- **Archivos**: `qwen_tts.h`, `qwen_tts.c`, `qwen_tts_server.c`.
- **Qué cambia**: `qwen_tts_set_abort_callback` registra un callback que `qwen_tts_generate` consulta al inicio de cada fotograma; si devuelve distinto de cero, el decodificador descarta lo pendiente y la generación devuelve `-1` sin audio. `handle_tts` lo registra con `client_disconnected`, que detecta el cierre del socket del cliente sin bloquear (`select` con timeout cero y `recv` con `MSG_PEEK`), y registra `[HTTP] TTS cancelado…` en vez de responder un error. El streaming también se cancela: `stream_http_callback` devuelve `-1` cuando un `write` falla.
- **Por qué**: el servidor atiende una conexión a la vez, ni siquiera `/v1/health` mientras genera. Sin esto, un trabajo abandonado por el cliente (plazo vencido, Ctrl+C o desconexión) lo bloquea hasta terminar.
- **Requisito del cliente**: no cerrar la mitad de escritura del socket tras enviar la petición; el motor lo interpretaría como un abandono y cancelaría la generación.
- **Si se pierde**: cada vencimiento deja el residente ocupado y el lanzador lo reemplaza en frío antes de la siguiente síntesis.

### Bloqueo del tokenizer y EOS de textos cortos

- **Archivos**: `qwen_tts_tokenizer.c` (validación UTF-8 y guarda de progreso), `qwen_tts.c` (refuerzo de EOS para textos cortos).
- **Qué cambia**: el tokenizer valida UTF-8 y aborta si no progresa; la generación refuerza el EOS en textos cortos.
- **Por qué**: ciertos textos colgaban el tokenizer y los textos cortos degeneraban en audio largo con ruido.
- **Si se pierde**: reaparecen el bloqueo del tokenizer y la degeneración de textos cortos.

### Poda de tooling y documentación del original

- **Archivos**: se eliminaron `blog/`, `docs/` (parcial), `tests/`, `tools/` y `training/`.
- **Qué cambia**: solo se conserva lo necesario para compilar y ejecutar el motor.
- **Por qué**: reducir el tamaño del snapshot.
- **Si se pierde**: no aplica; los objetivos del `Makefile` y los enlaces del `README.md` que dependían de esos directorios se podaron (ver «Poda de objetivos del `Makefile`, archivos vacíos y enlaces»).

### Corrección de la afirmación sobre AVX2 en `CLAUDE.md`

- **Archivos**: `CLAUDE.md`.
- **Qué cambia**: la descripción de `qwen_tts_kernels.c` indica que los bucles calientes sí contienen AVX2 y que las multiplicaciones cuantizadas usan un producto escalar entero (AVX2 `maddubs`, VNNI `dpbusd`, SDOT o bucle escalar entero) en todas las plataformas.
- **Por qué**: el texto original afirmaba que no había AVX2 en los bucles calientes.
- **Si se pierde**: la guía vuelve a describir mal los kernels; no afecta al binario.

### Eliminación de los backends GPU CUDA y Metal

- **Archivos**: se borraron `qwen_tts_cuda*`, `qwen_tts_metal*`, `qwen_tts_backend.{c,h}`, `bench_m2.sh`, `bootstrap_m2.sh` y `setup_m2.sh`; se retiraron los bloques `QWEN_HAVE_CUDA`/`QWEN_HAVE_METAL` y los hooks de GPU de `main.c`, `qwen_tts.c`, `qwen_tts_talker.c`, `qwen_tts_code_predictor.c`, `qwen_tts_speech_decoder.c`, `qwen_tts_kernels.{c,h}` y del `Makefile`.
- **Qué cambia**: el motor es solo CPU y su único target es `make blas`. Desaparecen los targets `make metal` y `make cuda` y los flags `--backend`, `--gpu-selftest`, `--gpu-selftest-talker` y `--gpu-batch-bench`.
- **Por qué**: ni el producto ni la CI compilaban esos backends y no se pueden compilar en el equipo de referencia, de modo que cualquier cambio de firma del decodificador los habría roto sin aviso. Una sola ruta de CPU permite optimizar el decodificador con el audio idéntico.
- **Si se pierde**: una actualización que reincorpore los backends añade código no compilable ni probado y obliga a mantener sus firmas sincronizadas con el decodificador de CPU; el lanzador no usa ninguno de esos flags.

### Eliminación de las ramas sin BLAS

- **Archivos**: `qwen_tts_speech_decoder.c`, `qwen_tts_kernels.c`, `qwen_tts_talker.c`, `qwen_tts_speech_encoder.c`, `Makefile`, `.github/workflows/codeql.yml`.
- **Qué cambia**: se retiraron las convoluciones `_naive`, los bucles escalares alternativos y los defines `USE_BLAS` y `USE_OPENBLAS`. El motor exige BLAS (Accelerate u OpenBLAS) y no existe una compilación sin él.
- **Por qué**: la ruta sin BLAS no se compilaba en el producto y duplicaba la lógica del decodificador, de modo que cada optimización había que escribirla dos veces.
- **Si se pierde**: reaparecen rutas muertas que divergen de la vigente y obligan a mantener dos implementaciones de cada convolución.

### Red convolucional unificada y streaming exacto único

- **Archivos**: `qwen_tts_speech_decoder.c`, `qwen_tts.h`.
- **Qué cambia**: la red convolucional del decodificador (ConvNeXt, convolución inicial, bloques de upsample y convolución final) era una copia inline en la decodificación completa, otra función usada solo por el streaming por ventanas y una tercera en el streaming exacto. Ahora la decodificación completa usa `conv_decoder_forward` y existe una única ruta de streaming con estado (colas de entrada por convolución y acarreos de solapamiento por convolución transpuesta), exacta respecto a la decodificación completa. Se eliminaron el streaming por ventanas, la variable `QWEN_SD_WINDOWED` y `QWEN_SD_STREAM_CONV_RF`. La cola del bloque ConvNeXt, `convnext_mlp`, es común a ambas rutas y su LayerNorm es de dos pasadas (media y luego varianza centrada).
- **Por qué**: el streaming por ventanas era aproximado en las uniones y hacía el triple de trabajo; con una sola red las optimizaciones se aplican una vez y el audio de producción (el streaming exacto) no cambia. La varianza de una pasada (`E[x²] - media²`) de la antigua copia inline habría alterado el audio del streaming.
- **Si se pierde**: vuelven tres copias de la red que pueden divergir entre sí; una LayerNorm de una pasada en `convnext_mlp` cambia el audio de producción respecto al validado.

### Espacio de trabajo persistente del decodificador

- **Archivos**: `qwen_tts.h` (`qwen_sd_workspace_t`, campo `sd_ws` del contexto), `qwen_tts_speech_decoder.c`, `qwen_tts.c`.
- **Qué cambia**: los búferes intermedios de la red convolucional viven en ranuras que solo crecen y se reutilizan entre bloques y entre peticiones, en lugar de reservarse y liberarse por bloque. La decodificación completa usa un espacio local, liberado al terminar, para no dejar ranuras del tamaño de toda la frase en el contexto. Los clones de worker lo inicializan a cero y tienen el suyo propio.
- **Por qué**: reservar y liberar decenas de MB por bloque obligaba en Windows a recibir páginas nuevas del sistema y a pagar su puesta a cero y su primer acceso en cada fragmento de streaming.
- **Si se pierde**: el decodificador vuelve a reservar memoria por bloque (más lento, mismo audio); compartir el espacio entre workers, en cambio, sería una condición de carrera.

### Convolución transpuesta con un solo GEMM

- **Archivos**: `qwen_tts_speech_decoder.c` (`causal_conv_transpose1d`).
- **Qué cambia**: la convolución transpuesta causal calcula un único GEMM sobre el peso `[entrada][salida·k]` (contiguo tal como está en el archivo) y dispersa el producto sobre la salida con zancada en orden ascendente de `k`.
- **Por qué**: la versión original reordenaba una copia de los pesos en cada llamada antes de multiplicar. Con una sola multiplicación sobre la vista contigua desaparece ese trabajo y su consumo de ancho de banda; el audio varía como máximo 1 LSB (SNR 96,6-98,6 dB, con WER y similitud WavLM idénticos).
- **Si se pierde**: el decodificador es más lento y le quita ancho de banda de memoria al Code Predictor; el audio es inaudiblemente distinto.

### Convolución causal sin im2col con pesos `W_k` precalculados

- **Archivos**: `qwen_tts_speech_decoder.c` (`causal_conv1d`, `conv_reorder_weight`, `qwen_speech_decoder_free_conv_weights`), `qwen_tts.h` (`pre_conv_wk`, `initial_conv_wk`, `conv1_wk`), `qwen_tts.c` (la descarga del contexto base libera los pesos reordenados).
- **Qué cambia**: la convolución causal de kernel mayor que 1 acumula k GEMM sobre la entrada con relleno izquierdo de ceros, sin materializar la matriz im2col. Los pesos se reordenan una vez al cargar a `[k][salida][entrada]` (14 matrices, unos 116 MB) y los clones de worker los comparten. La ruta INT8 experimental (`QWEN_SD_INT8`) conserva su propio im2col.
- **Por qué**: la versión original copiaba la entrada una vez por posición del kernel en una tabla im2col troceada. Esa copia y el troceo (`CONV_TILE_MAX_BYTES`) desaparecen; el audio coincide con 1 LSB como máximo.
- **Si se pierde**: la convolución vuelve a ser más lenta y a consumir ancho de banda de memoria; si un worker liberase los pesos compartidos, los demás leerían memoria liberada.

### Afinidad de un hilo lógico por núcleo físico en Windows

- **Archivos**: `main.c` (`pin_one_thread_per_core`, bloque `_WIN32`).
- **Qué cambia**: antes de crear los pools de hilos, el proceso se restringe con `SetProcessAffinityMask` a un hilo lógico por núcleo físico. La máscara sale de la topología (`GetLogicalProcessorInformationEx`; p. ej. `0x5555` en un Ryzen 7 5825U), se interseca con la afinidad heredada y no se fija nada si hay varios grupos de procesadores.
- **Por qué**: con SMT el planificador de Windows coloca a veces dos hilos pesados en el mismo núcleo y el RTF salta de ~1,1 a 1,3-1,5 de forma intermitente; el audio no cambia.
- **Si se pierde**: reaparece ese estado lento intermitente en equipos Windows con SMT.

### Enlace de `test-decoder-tool` con ingot

- **Archivos**: `Makefile` (target `test-decoder-tool`).
- **Qué cambia**: el target enlaza `libingot.a`, igual que el binario principal.
- **Por qué**: el codificador y el decodificador leen los pesos con las funciones de ingot, y sin la librería el enlace de `qwen_tts_decoder_tool` fallaba con referencias sin resolver.
- **Si se pierde**: `make test-decoder-tool` deja de enlazar y no se puede validar la decodificación completa con un volcado fijo de códigos.

### Espera activa del pool de hilos en Windows

- **Archivos**: `qwen_tts_thread.c` (pool Win32, `qwen_pool_spin`, `qwen_cpu_relax`).
- **Qué cambia**: los workers del pool Win32 giran sobre la generación del trabajo durante `QWEN_POOL_SPIN` iteraciones (4096 por defecto) antes de dormir en el evento, igual que el pool pthread; `qwen_pool_spin()` y `qwen_cpu_relax()` son comunes a ambos pools.
- **Por qué**: las multiplicaciones cuantizadas duran microsegundos y despertar un hilo dormido en Windows cuesta más que la propia tarea; sin espera activa el reparto entre hilos se pierde en latencia de despertar.
- **Si se pierde**: el pool Win32 vuelve a dormir en cada tarea y el Talker y el Code Predictor son más lentos en Windows, sin cambio de audio.

### Producto escalar entero en AVX2 y en la ruta escalar

- **Archivos**: `qwen_tts_kernels.c` (`quantize_act_int8_ref`, `quantize_act_int8`, `quantize_act_int8_cols`, `int8_matvec_avx2`, `q4_0_matvec_avx2`, sus equivalentes escalares y `qwen_caps_report`), `qwen_tts_kernels.h`.
- **Qué cambia**: `qwen_matvec_int8`, `qwen_argmax_matvec_int8`, `qwen_matvec_q4_0` y `qwen_matvec_q4_0_qkv` cuantizan la activación a int8 con un único cuantizador (escala máx|x|/127, redondeo al par, saturación a [-128,127]; versiones AVX2, NEON y C con resultado idéntico) y multiplican con producto escalar entero en todas las ISA: AVX2 `maddubs` (int8 con `sign` + `maddubs` + `madd`, dos filas a la vez; q4 con nibbles sin signo × activación con signo, corrección de 8·Σqa y escala fp16 por bloque), VNNI `dpbusd`, SDOT y, sin SIMD, un bucle con acumulación int32. El núcleo q4 de AVX2 hace prefetch por software de 16 bloques por delante en cada una de las dos filas. `--caps` declara el producto escalar entero por ISA.
- **Por qué**: el original dequantizaba cada peso a float y multiplicaba con FMA, lo que dejaba el cómputo por encima del ancho de banda en CPUs sin VNNI ni SDOT. El producto entero reduce el cómputo (en un Ryzen 7 5825U con Talker Q4_0 y Code Predictor INT8 el RTF sin carga baja de 0,93 a 0,76) y todas las plataformas comparten el mismo algoritmo. El prefetch evita que el núcleo q4, ahora más rápido que el flujo de memoria, espere un fallo de caché por bloque cuando el bus está saturado; no cambia ningún valor numérico.
- **Si se pierde**: vuelve el producto en coma flotante, más lento y con resultados distintos según la ISA; sin el prefetch el Talker empeora con el bus de memoria ocupado.

### `matmat` entero y elección fija de kernel en ARM

- **Archivos**: `qwen_tts_kernels.c` (`qwen_matmat_int8`, `qwen_matmat_q4_0`, `matmat_via_matvec_*`, `matmat_alloc`, `matmat_oom`), `qwen_tts_kernels.h`.
- **Qué cambia**: `qwen_matmat_int8` y `qwen_matmat_q4_0` cuantizan cada columna con el cuantizador por vector del matvec y multiplican con producto escalar entero. En AVX2 y escalar cada columna es bit a bit la de B llamadas a matvec (int8 y q4); en VNNI int8 es exacto y q4 acumula por pares de bloques; ARM con dotprod sin i8mm (y Apple en int8) hace B matvec; i8mm int8 se usa fuera de Apple y no en Apple, y q4 i8mm se usa siempre (difiere ~1e-7 de SDOT por el orden de acumulación). La elección es fija en tiempo de compilación, sin interruptores de entorno. Un fallo de memoria en un matmat agrupado imprime un error y llama a `abort()`.
- **Por qué**: con el mismo cuantizador y la misma función de fila, una petición agrupada calcula igual que una individual (el audio no depende del tamaño del lote), y no hay rutas de respaldo que degraden el resultado sin avisar.
- **Si se pierde**: el matmat agrupado da resultados distintos del matvec según la ISA y vuelven las rutas elegibles por entorno.

### PREFILL2 como ruta única instrumentada

- **Archivos**: `qwen_tts_code_predictor.c` (`cp_prefill2_mode`, macro `CPB_MARK`).
- **Qué cambia**: el prefill del Code Predictor usa siempre la ruta PREFILL2 con CP uniforme int8 o q4 (`cp_prefill2_mode` devuelve 1, 2 o 0 si no aplica), sin la variable `QWEN_CP_PREFILL2`, y sus fases están instrumentadas con `CPB_MARK`.
- **Por qué**: como matmat equivale a B matvec, la ruta agrupada da el mismo resultado que la secuencial y puede ser la única; la instrumentación permite medir cada fase.
- **Si se pierde**: reaparece una segunda ruta de prefill seleccionable por entorno y sin garantía de igualdad con la vigente.

### Exigencia de `dotprod` en Linux ARM64

- **Archivos**: `Makefile` (`ARCH_FLAGS`, `ISACHK`, `check-isa`), `qwen_tts_kernels.c` (`#error` inicial).
- **Qué cambia**: en Linux ARM64 el motor se compila por defecto con `-march=armv8.2-a+dotprod` (`SIMD=native` usa `-march=native`) y `qwen_tts_kernels.c` emite `#error` en ARM64 sin `__ARM_FEATURE_DOTPROD`. `check-isa` reconoce `arm64` y `aarch64`, comprueba la línea base `dotprod` y el rechazo de `armv8-a`, y desde x86 compila las rutas ARM con clang cruzado (`ISACHK_ARM_X`); `ISACHK` hereda las banderas de plataforma de `CFLAGS_BASE`.
- **Por qué**: todas las multiplicaciones cuantizadas de ARM usan SDOT; no existe ruta de respaldo sin él, así que la ausencia debe fallar al compilar y no degradar en silencio.
- **Si se pierde**: una compilación para `armv8-a` deja de fallar con un error claro y `check-isa` no detecta el fallo.

### Comprobación de dimensiones al cargar el modelo

- **Archivos**: `qwen_tts_kernels.c` y `qwen_tts_kernels.h` (`qwen_quant_shape_check`), `qwen_tts_talker.c` (`qwen_talker_load`), `qwen_tts_code_predictor.c` (`qwen_cp_load`).
- **Qué cambia**: al cargar el modelo se comprueba, de forma incondicional, que las dimensiones de entrada de las matrices cuantizadas (tamaño oculto, dimensión de Q e intermedia del Talker y del Code Predictor) sean múltiplo de 32 y no mayores que 8192 (`QWEN_QACT_MAX`); si no, la carga falla con un error que nombra la dimensión. Las multiplicaciones no comprueban nada por llamada y cuantizan la activación en un búfer de pila de ese tamaño.
- **Por qué**: un modelo con otras dimensiones desbordaría el búfer de pila o leería bloques incompletos; fallar una vez al cargar es más barato y más claro que comprobar en cada llamada.
- **Si se pierde**: un modelo con dimensiones no soportadas corrompe la pila o da resultados erróneos sin error.

### Eliminación de la coma flotante, de los interruptores y de la escalera VNNI

- **Archivos**: `qwen_tts_kernels.c`, `qwen_tts_code_predictor.c`, `Makefile`.
- **Qué cambia**: se eliminaron los kernels en coma flotante de int8 y q4 (`int8_matvec_fused`, `q4_0_matvec_inner` y similares), las variables de entorno `QWEN_NO_SDOT`, `QWEN_NO_VNNI`, `QWEN_NO_VNNI_QKV`, `QWEN_INT8_SDOT_MM`, `QWEN_NO_SMMLA`, `QWEN_APPLE_MMLA`, `QWEN_CP_PREFILL2`, `QWEN_Q4_VNNI_V3` y `QWEN_Q4_VNNI_V4`, y la escalera de versiones VNNI de q4: queda v3 (cuatro filas, dos bloques por `dpbusd`), con v2 para las filas sobrantes. `test-selftest` se ejecuta en una pasada y `test-modes` comprueba 4 combinaciones. Se conservan `QWEN_NO_BF16DOT`, `QWEN_NO_BFMMLA` y `QWEN_NO_SIN_POLY`.
- **Por qué**: cada interruptor y cada versión alternativa dejaba una ruta sin cubrir por las pruebas y capaz de degradar en silencio; una sola ruta entera por ISA es la que se valida y se mide.
- **Si se pierde**: reaparecen rutas alternativas seleccionables por entorno con numérica distinta de la validada.

### Poda de objetivos del `Makefile`, archivos vacíos y enlaces

- **Archivos**: `Makefile`, `README.md`, `qwen_tts_kernels_avx.c`, `qwen_tts_kernels_neon.c`, `qwen_tts_kernels_generic.c` y `qwen_tts_kernels_impl.h` (eliminados).
- **Qué cambia**: se eliminaron veinte objetivos del `Makefile` sin fuentes en el snapshot (`emotion-demo`, `emotion-para-demo`, `emovoice`, `emo-06b-demo`, `para-demo`, `emo-suite`, `test-lora-it`, `emotion-seeds`, `batching-bench`, `bench-matrix`, `bench-matrix-full`, `bench-server`, `test-golden`, `golden-update`, `quant-ladder`, `test-serve-concurrent`, `test-serve-repro`, `test-serve-batch`, `test-serve-continuous`, `test-serve-stream-batch`), los archivos `qwen_tts_kernels_{avx,neon,generic}.c` y `qwen_tts_kernels_impl.h` (sin código) y los enlaces del `README.md` a `docs/`, `blog/` y `samples/` (se conservan las URLs externas a `raw/main/samples/`). `demo-clone` exige `REF=`.
- **Por qué**: el snapshot no contiene las fuentes ni los documentos de esos objetivos y enlaces, de modo que fallaban o apuntaban a la nada.
- **Si se pierde**: reaparecen objetivos que no pueden ejecutarse y archivos vacíos que aparentan una estructura de kernels que no existe.
