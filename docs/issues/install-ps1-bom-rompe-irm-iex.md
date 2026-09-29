# La instalación de una línea en Windows (`irm | iex`) falla con `ParserError` por el BOM de `install.ps1`

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | crítica |
| Tipo | funcional |
| Componente | `packaging/bootstrap/install.ps1` (asset `install.ps1` del Release) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11, Windows PowerShell 5.1 y PowerShell 7.6. No afecta a `install.sh` |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

El comando de instalación que publica el README para Windows no llega a ejecutar
nada: PowerShell rechaza el script al parsearlo. El asset `install.ps1` empieza con un
BOM UTF-8 que, al descargarse como texto con `irm` y pasarse a `iex`, deja de
interpretarse como marca de codificación y se convierte en caracteres dentro del código.
Es la puerta de entrada al producto en Windows, así que un usuario nuevo no puede
instalarlo por el canal anunciado.

## Entorno

- Binario: v0.25.0 (Release de GitHub `latest`).
- SO: Windows 11 Home 10.0.26200.
- Shells: Windows PowerShell 5.1 y PowerShell 7.6.6.
- Sin instalación previa, sin daemon, sin modelos.

## Precondiciones

Ninguna más allá de acceso a GitHub. El fallo ocurre antes de que el script haga nada.

## Pasos para reproducir

1. Abrir Windows PowerShell 5.1 (o `pwsh` 7).
2. Ejecutar el one-liner del README:

   ```powershell
   irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
   ```

## Resultado observado

`iex` aborta con `ParserError` al principio del script, con un mensaje del estilo
«El operador '<' está reservado para uso futuro». Exit 1. No se descarga ni se instala
nada. El mismo error aparece en PowerShell 5.1 y en 7.6.

## Resultado esperado

El README presenta `irm … | iex` como la instalación estándar en Windows: el bootstrap
descarga el `.zip`, verifica el hash, comprueba el arranque y delega en `self install`.
Debe completar la instalación con exit 0 en PowerShell 5.1 y 7.

## Impacto y workaround

Afecta a toda instalación nueva en Windows por el canal documentado.

Workaround: descargar el archivo y ejecutarlo como archivo, porque así PowerShell sí
consume el BOM:

```powershell
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Con el BOM retirado, el mismo script instaló v0.25.0 con exit 0 en PowerShell 5.1.

## Evidencia

- Los tres primeros bytes de `packaging/bootstrap/install.ps1` son `EF BB BF` (BOM
  UTF-8) y el archivo tiene 25 líneas con caracteres no ASCII.
- En PowerShell 5.1, la cadena que devuelve `irm` empieza por los códigos
  `239, 187, 191`: `irm` decodificó la respuesta como Latin-1/ANSI y el BOM se
  convierte en tres caracteres visibles. En PowerShell 7, la cadena empieza por `U+FEFF`.
- En los dos casos, `iex` recibe el texto con esos caracteres delante de `<#`. `<#` deja
  de abrir el bloque de comentario y el parser ve un `<` suelto.
- En PowerShell 7, aplicar `TrimStart([char]0xFEFF)` a la cadena antes de `iex` hace que
  el script parsee y se ejecute.
- El job de publicación de CircleCI estampa la versión con `sed` y copia el archivo a
  `artifacts/` tal cual, así que el asset conserva el BOM del repositorio.

## Análisis de causa

1. **Confirmada.** El BOM se añadió a propósito para que PowerShell 5.1 leyera bien los
   acentos al **ejecutar el archivo desde disco** (entrada del CHANGELOG «añadir BOM
   UTF-8 a los `.ps1` para PowerShell 5.1»). Esa decisión es correcta para `-File`, pero
   rompe el flujo `irm | iex`, donde el contenido llega como cadena y el BOM ya no es
   metadato.
2. **Confirmada.** Las pruebas no detectan el fallo. `tests/bootstrap/install.tests.ps1`
   simula `irm | iex` leyendo el script con `Get-Content -Raw` y pasándolo por stdin
   (`Invoke-ChildBootstrap -StdinText`, en `tests/bootstrap/support/Setup.ps1`), pero
   `Get-Content` consume el BOM al leer. La cadena que recibe la prueba nunca lo lleva,
   a diferencia de la que devuelve `irm`.
3. **Probable.** En PowerShell 5.1 se suma otro problema: aunque se quite el BOM, `irm`
   puede decodificar el cuerpo como ANSI si el servidor no declara `charset=utf-8`, y
   los caracteres no ASCII de los comentarios y mensajes saldrían corruptos. Eso no
   rompe el parseo si esos caracteres solo están en comentarios y cadenas, pero conviene
   comprobarlo.

## Diagnóstico sugerido

Hay tres direcciones de corrección:

- **Asset sin BOM y solo ASCII.** Quitar el BOM y dejar el script en ASCII puro (sin
  acentos en comentarios ni mensajes, o con los mensajes construidos con
  `[char]0x00E1`). Elimina el problema en los dos flujos y en las dos versiones de
  PowerShell.
- **BOM en el repositorio, retirado al publicar.** Mantener el BOM en el repositorio y
  quitarlo en el paso de estampado de CI. Tiene el riesgo del punto 3 en PowerShell 5.1.
- **Recortar el BOM dentro del propio script.** No sirve: el script no llega a
  ejecutarse.

Antes de decidir, conviene probar en PowerShell 5.1 la variante sin BOM que conserva
acentos, para medir el riesgo del punto 3.

## Criterio de aceptación

- El asset `install.ps1` publicado no empieza por `EF BB BF`.
- `irm <url> | iex` instala con exit 0 en PowerShell 5.1 y en 7.
- Existe una prueba de regresión que alimenta `iex` con los **bytes crudos** del archivo
  (por ejemplo, `[IO.File]::ReadAllBytes` decodificado igual que lo hace `irm`) en lugar
  de `Get-Content`, y que falla con el BOM actual.
- Una comprobación de CI rechaza el BOM en `packaging/bootstrap/install.ps1`, si esa es
  la vía elegida.

## Relacionados

- Entrada del CHANGELOG que introdujo el BOM en los `.ps1` del instalador.
- README, sección de instalación en Windows.
