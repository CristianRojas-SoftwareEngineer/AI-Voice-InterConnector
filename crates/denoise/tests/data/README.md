# Datos de test

Vectores PCM16 deterministas para `test_golden`.

## Generación

Los vectores del par de 3 s y del par de 65 s se generan con
`examples/gen_vectors.rs`, que necesita las fuentes
`spanish_street_*`/`spanish_subway_*` del conjunto de evaluación:

```bash
cargo run --release --example gen_vectors -- <dir_eval>
```

donde `<dir_eval>` contiene `Clean/` y `Noisy/` del conjunto de evaluación de
DPDFNet (`Ceva-IP/DPDFNet_EvalSet`, Apache 2.0). Nunca se generan en
`cargo test` ni CI: los archivos están congelados.

## Modelo

Las referencias se generaron con **DPDFNet 48 kHz HR**
(`dpdfnet8_48khz_hr.onnx`, Ceva-IP/DPDFNet, Apache 2.0) ejecutado por este
crate y se congelaron como línea base de regresión. La referencia del par de
3 s es la salida previa al gate de pausa. La del par de 65 s incluye el gate de
pausa: se regenera con `examples/process_wav.rs` sobre `voz65s_noisy.wav`
(mismo camino `denoise_wav`), sin necesidad del conjunto de evaluación. La
paridad con sherpa-onnx (1 LSB PCM16) se verificó en el pipeline previo al
gate; con el gate activo la voz es bit-idéntica (×1.0), pero la salida completa
no, porque las pausas se atenúan −25 dB tras la síntesis. Ambos dorados pasan
con 67,77 dB y 100,00 dB, sobre el umbral de ≥60 dB.

## Archivos

- `voz_clean.wav` - voz real 3s, SR 48k, mono PCM16 (ventana de máxima energía)
- `voz_noisy.wav` - `voz_clean.wav` + ruido a SNR 0 dB
- `referencia_dpdfnet.wav` - salida de DPDFNet sobre `voz_noisy.wav`, congelada
- `voz65s_clean.wav` - voz real 65s, SR 48k, mono PCM16
- `voz65s_noisy.wav` - `voz65s_clean.wav` + ruido a SNR 10 dB
- `referencia65s_dpdfnet.wav` - salida de DPDFNet sobre `voz65s_noisy.wav`, congelada

> **Nota:** Estos archivos son deterministas y se mantienen congelados.
> `gen_vectors` congelaría las referencias **con** el gate activo, así que la
> referencia del par de 3 s (previa al gate) no se reproduce por ese camino.
