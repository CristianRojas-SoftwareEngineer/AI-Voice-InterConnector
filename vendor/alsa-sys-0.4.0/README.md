# alsa-sys

Rust raw FFI bindings for ALSA.

## Parche de esta copia vendorizada

Esta copia no enlaza la librería de ALSA al compilar, de modo que el binario
no declara `libasound.so.2` como dependencia y compilar no exige sus cabeceras
ni `pkg-config`:

- Los bindings están fijados en `src/generated.rs`. `build.rs` los reescribe en
  `OUT_DIR/bindings.rs`: los tipos quedan iguales y cada función externa se
  convierte en una envoltura con la misma firma que resuelve su símbolo con
  `dlsym` sobre `libasound.so.2`, cargada con `dlopen` en el primer uso. Las
  funciones variádicas no se pueden reenviar desde Rust estable y se omiten.
- `src/required_symbols.rs` lista las funciones que llama el crate `alsa`.
  `library_available()` comprueba que la librería carga y las define todas, y
  `missing_symbol()` devuelve la primera que falta, para que quien use el crate
  falle con un error propio en vez de abortar en mitad de una llamada.
- La compilación falla si la salida conserva algún bloque `extern "C"` o si la
  lista nombra una función sin envoltura.

## Regenerar la lista de funciones requeridas

Al subir de versión el crate `alsa`, la lista se regenera con las funciones
`snd_*` de `src/generated.rs` que su código llama, ya sea como `alsa::snd_*` o
dentro de `acheck!(snd_*(…))`, macro que antepone `alsa::`. `build.rs` rechaza
cualquier nombre que no tenga envoltura.
