# alsa-sys

Rust raw FFI bindings for ALSA.

## Parche de esta copia vendorizada

Esta copia no enlaza la librería de ALSA al compilar, de modo que el binario
no declara `libasound.so.2` como dependencia y compilar no exige sus cabeceras:

- Los bindings están fijados en `src/generated.rs`. `build.rs` los reescribe en
  `OUT_DIR/bindings.rs`: los tipos quedan iguales y cada función externa se
  convierte en una envoltura con la misma firma que resuelve su símbolo con
  `dlsym` sobre `libasound.so.2`, cargada con `dlopen` en el primer uso. Las
  funciones variádicas no se pueden reenviar desde Rust estable y se omiten.
  Cada envoltura guarda su dirección en un estático con
  `#[export_name = "avi_alsa_fn_<función>"]`: rustc lo emite en su propia
  sección y el enlazador solo lo conserva si el binario alcanza esa envoltura.
- `src/required_symbols.rs` lista las funciones que alcanza el binario.
  `availability()` comprueba que la librería carga y las define todas, y si no,
  devuelve `Unavailable::Library` o `Unavailable::Symbol` con la primera que
  falta, para que quien use el crate falle con un error propio en vez de
  abortar en mitad de una llamada.
- La compilación falla si la salida conserva algún bloque `extern "C"`.

## Lista de funciones requeridas

`src/required_symbols.rs` es un fichero generado. `cargo xtask alsa-symbols
--binary <binario Linux>` lo regenera desde la tabla de símbolos del binario
enlazado, y con `--check` falla si no coincide exactamente. La comprobación
corre en el job `test-linux` (binario de pruebas) y en la auditoría de
`cargo xtask package` (binario que se publica).
