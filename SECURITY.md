# Política de seguridad

## Tabla de contenidos

- [Versiones soportadas](#versiones-soportadas)
- [Cómo reportar una vulnerabilidad](#cómo-reportar-una-vulnerabilidad)
- [Modelo de amenaza](#modelo-de-amenaza)
  - [Daemon local (modo daemon)](#daemon-local-modo-daemon)
  - [Modelo y provisión](#modelo-y-provisión)
  - [Contenido generado](#contenido-generado)
  - [Nota sobre los instaladores de una línea](#nota-sobre-los-instaladores-de-una-línea)
- [Artefactos sin firmar](#artefactos-sin-firmar)
  - [Runbook: reportar un falso positivo de Defender Antivirus (WDSI)](#runbook-reportar-un-falso-positivo-de-defender-antivirus-wdsi)
  - [Runbook: reportar un falso positivo de ClamAV](#runbook-reportar-un-falso-positivo-de-clamav)
  - [Runbook: bloqueo de Gatekeeper en macOS](#runbook-bloqueo-de-gatekeeper-en-macos)

## Versiones soportadas

AI Voice InterConnector está en desarrollo activo pre-1.0. Solo la última versión publicada
recibe correcciones de seguridad.

| Versión | Soportada |
|---------|-----------|
| 0.1.x   | ✅ |
| < 0.1.0 | ❌ |

## Cómo reportar una vulnerabilidad

**No** reportes vulnerabilidades de seguridad en Issues públicos.

Usa el canal privado de
[**GitHub Security Advisories**](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/security/advisories/new)
para reportarlas de forma confidencial. Incluye:

- Una descripción de la vulnerabilidad y su impacto.
- Pasos para reproducirla (versión, SO, comando o entrada).
- Cualquier mitigación conocida.

Recibirás una respuesta inicial en un plazo razonable. Te pedimos no divulgar
públicamente el problema hasta que exista una corrección disponible.

## Modelo de amenaza

AI Voice InterConnector es una herramienta **local y offline**. Su superficie de ataque es
limitada, pero conviene explicitar sus supuestos:

### Daemon local (modo daemon)

- El daemon escucha **únicamente en loopback** (`127.0.0.1:8765`); no se expone a la
  red. **No implementa autenticación**: delega el control de acceso en el sistema
  operativo (cualquier proceso local del mismo usuario puede invocarlo).
- **No ejecutes el daemon en un host multiusuario no confiable** esperando
  aislamiento entre usuarios locales: no lo provee.
- El endpoint de apagado (`POST /shutdown`) **no exige token ni confirmación**:
  cualquier proceso local del mismo usuario puede apagar el daemon. Es una brecha
  de defensa en profundidad de bajo impacto (un DoS local que el mismo usuario ya
  puede provocar matando el proceso por PID), no un fallo de correctness. Se acepta
  ese riesgo residual en lugar de introducir un secreto que el propio cliente IPC
  tendría que generar y persistir, sin ganancia real bajo un binding exclusivo a
  loopback y modelo single-user.
- El endpoint de síntesis valida la entrada (`text` acotado a 5000 caracteres)
  antes de procesarla.
- No reenvíes ni expongas el puerto 8765 a interfaces de red externas.

### Modelo y provisión

- El modelo se descarga desde HuggingFace con `ai-voice-interconnector setup` a la caché local,
  con la **revisión fijada por release** (commit hash auditado, declarado en
  `src/ai_voice_interconnector/model_cache.py`): un push posterior al repo del modelo —
  malicioso o accidental — no se propaga a los usuarios, y la detección de caché
  solo valida el snapshot de esa revisión en ambos repos (language pack y repo
  base). El alcance del pin es ese: protege contra cambios posteriores a la
  revisión auditada; no es una verificación criptográfica por archivo de los
  pesos descargados (el transporte y el direccionamiento por hash de commit
  corren a cargo de HuggingFace Hub).
- Los builds se producen desde un **lockfile con hashes** (`--require-hashes`), lo
  que protege la cadena de dependencias frente a paquetes alterados en PyPI.

### Contenido generado

- El audio sintetizado **no lleva marca de agua** (el watermark de PerthNet está
  desactivado). Esto es una decisión de diseño con implicaciones de uso responsable,
  no una vulnerabilidad; ver «Uso ético y responsable» en
  [README.md](README.md)/[USAGE.md](USAGE.md).

### Nota sobre los instaladores de una línea

Los tres instaladores de una línea (`install-linux.sh` en Linux, `install-macos.sh`
en macOS, `install-windows.ps1` en Windows; raíz del repo) se sirven por
`raw.githubusercontent.com` y se ejecutan con `curl | sh` / `irm | iex`, el
patrón habitual de instalación de una línea. La mitigación de ese patrón
—ejecutar contenido remoto sin inspeccionarlo antes— es común a los tres: **el
propio script verifica el checksum SHA-256** del artefacto descargado (el
`.AppImage`, el `.dmg` o el `.exe`) contra `SHA256SUMS.txt` (publicado junto al
Release, ver «Artefactos sin firmar» abajo) **antes** de darle permisos de
ejecución, montarlo o invocarlo; un checksum que no coincide aborta la
instalación sin ejecutar nada. Ninguno requiere privilegios elevados:

- **Linux** (`install-linux.sh`): instala en `~/.local/opt/ai-voice-interconnector/`, sin `sudo`.
- **macOS** (`install-macos.sh`): verifica con `shasum -a 256 -c`, copia el
  `.app` a `~/Applications` y crea el symlink en `~/.local/bin`, sin `sudo`.
  Limpia el atributo `com.apple.quarantine` del `.app` copiado (legítimo: el
  usuario ya expresó intención al ejecutar el script), lo que evita la
  advertencia de Gatekeeper en el primer arranque.
- **Windows** (`install-windows.ps1`): instalación per-user
  (`%LOCALAPPDATA%\Programs\ai-voice-interconnector`, PATH en `HKCU\Environment`), sin UAC.
  El navegador sella todo archivo descargado con el Mark-of-the-Web, la marca
  que dispara SmartScreen; la descarga por CLI (`Invoke-WebRequest`) no la
  aplica, por lo que el instalador descargado por el script no dispara
  SmartScreen. Microsoft Defender **Antivirus** es independiente del MOTW y
  puede marcar el binario sin firma — en ese caso aplica el runbook WDSI de
  más abajo. Como `irm | iex` no escribe un `.ps1` en disco, no pasa por la
  Execution Policy; la alternativa inspeccionable es
  `iwr <url> -OutFile install-windows.ps1; .\install-windows.ps1`.

## Artefactos sin firmar

Los binarios distribuidos **no están firmados ni notarizados**: Gatekeeper (macOS) y
SmartScreen (Windows) advierten en el primer arranque, y cada release —al ser
un archivo nuevo sin reputación acumulada— vuelve a disparar la advertencia.

Ambos avisos dependen de una marca que **solo añade el navegador** al descargar
un archivo de internet: en Windows es el *Mark-of-the-Web* (MOTW, `ZoneId=3`),
en macOS el atributo extendido `com.apple.quarantine`. Una descarga por línea
de comandos (`curl`, `Invoke-WebRequest`, `gh`) no la aplica, y un binario
generado localmente (como el launcher del canal PyPI, ver
[docs/DISTRIBUTION.md](docs/DISTRIBUTION.md)) tampoco la lleva nunca; por eso
los instaladores de una línea de este proyecto no disparan la advertencia
aunque el binario siga sin firmar. La firma de código (ver «Ruta prevista»
abajo) es el arreglo de fondo: sustituye la advertencia de reputación por una
confirmación de identidad del editor, independientemente del medio de
descarga.

Sin firma de código, la única verificación de integridad disponible es cotejar
el SHA-256 del artefacto descargado contra el archivo `SHA256SUMS.txt`
publicado junto a cada
[Release](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases)
— esos hashes, a su vez, deben coincidir con los emitidos en el log del
pipeline de CI que construyó el artefacto. Ver el flujo completo de
publicación y verificación en [docs/RELEASING.md](docs/RELEASING.md), y la
guía de usuario para los diálogos de bloqueo en
[USAGE.md](USAGE.md#el-sistema-bloquea-el-primer-arranque-binarios-sin-firmar).

**Ruta prevista**: incorporar firma de código vía
[SignPath Foundation](https://signpath.org/), que ofrece firma gratuita a
proyectos open source con licencia OSI (el certificado se emite a nombre de la
fundación y la clave privada reside en su HSM; requiere builds verificables
desde el repositorio, MFA de los mantenedores y una política de firma
publicada). La firma OV no suprime la advertencia de SmartScreen de inmediato
—la reputación se acumula por volumen de descargas— pero reemplaza «editor
desconocido» por un editor verificable.

Mientras tanto, el build aplica dos mitigaciones baratas sin firma de código:
todos los artefactos de PyInstaller se empaquetan con `--noupx` (sin
compresión UPX, una de las señales que la heurística antivirus asocia con
malware), y el `.exe` de Windows lleva metadata PE de identidad (empresa,
producto y versión vía `--version-file`), ambas cubiertas por test en
`tests/test_build_utils.py` y `tests/test_build_windows.py`.

### Runbook: reportar un falso positivo de Defender Antivirus (WDSI)

Si un release de Windows es marcado por **Microsoft Defender Antivirus** (por
ejemplo con una firma como `Trojan:Win32/Wacatac`), reporta el falso positivo
al portal de Microsoft (*Windows Defender Security Intelligence*):

1. Entra a [microsoft.com/wdsi](https://www.microsoft.com/en-us/wdsi/filesubmission)
   y elige **"Submit a file for malware analysis"**.
2. Sube el artefacto marcado (el `.exe` o el instalador Inno Setup) tal cual se
   descargó del [Release](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases);
   el reporte funciona con el binario **sin firmar**, no hace falta esperar a
   tener firma de código.
3. Clasifica la muestra como **"Incorrectly detected as malware / False
   Positive"** e indica la detección exacta que reportó Defender.
4. En el campo de comentarios, aclara que es un ejecutable PyInstaller de
   código abierto (enlaza este repositorio y el commit/tag del que se
   construyó el artefacto, verificable contra `SHA256SUMS.txt` — ver
   [docs/RELEASING.md](docs/RELEASING.md)).
5. Microsoft revisa la muestra con un analista y, si confirma el falso
   positivo, **borra la detección globalmente** para todos los usuarios de
   Defender (no solo para quien reportó).

Alcance de este runbook: cubre **únicamente** la detección de Defender
Antivirus (ver también los runbooks de ClamAV y Gatekeeper abajo). **No**
desactiva ni acelera el paso de SmartScreen — SmartScreen es
reputación de archivo/editor, y solo la resuelve la firma de código (ver la
ruta prevista arriba), no un reporte a WDSI. Firmar el binario tampoco borra
retroactivamente una detección de Defender ya existente: solo el reporte a
WDSI lo hace. Sin firma de código, la reputación se acumula por archivo
individual, así que **cada versión nueva puede requerir un reporte propio**;
con firma de código, la reputación se hereda entre versiones y esa
recurrencia disminuye considerablemente. Ver también la guía de usuario para
los diálogos de bloqueo en
[USAGE.md](USAGE.md#el-sistema-bloquea-el-primer-arranque-binarios-sin-firmar).

### Runbook: reportar un falso positivo de ClamAV

ClamAV (usado sobre todo en Linux y en pasarelas de correo) puede marcar el
binario PyInstaller como `PUA.Win32.Packer.PyInstaller` u otra detección de
empaquetador, especialmente el AppImage/onedir de Linux. No hay un portal
central de falsos positivos equivalente al de Microsoft; el reporte se hace al
proyecto ClamAV:

1. Confirma la detección con `clamscan --verbose <ruta-del-artefacto>` y anota
   la firma exacta que reportó (p. ej. `PUA.Win32.Packer.PyInstaller-*`).
2. Verifica la integridad del archivo cotejando su SHA-256 contra
   `SHA256SUMS.txt` del [Release](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases)
   (ver [docs/RELEASING.md](docs/RELEASING.md)): un hash que coincide descarta
   que el archivo haya sido alterado en tránsito.
3. Reporta la firma como falso positivo a ClamAV vía su rastreador de
   incidencias / portal de muestras (Talos/Cisco), adjuntando el artefacto y
   aclarando que es un ejecutable PyInstaller de código abierto (enlaza este
   repositorio y el commit/tag del que se construyó).
4. Mientras la firma no se actualiza, el usuario afectado puede excluir la ruta
   o el archivo de su escaneo local (p. ej. `clamscan --exclude=<ruta>`) tras
   confirmar el paso 2.

### Runbook: bloqueo de Gatekeeper en macOS

Gatekeeper no es una heurística de malware sino una **puerta de firma y
notarización**: al no estar el binario firmado ni notarizado, macOS lo pone en
cuarentena en el primer arranque. El diálogo típico es «no se puede abrir
porque el desarrollador no puede ser verificado». No existe un portal de
«falso positivo» de Gatekeeper; la solución definitiva es la firma de código
notarizada (ver la *ruta prevista* de SignPath más arriba). Mientras tanto, el
usuario puede desbloquear el binario:

1. En `System Settings → Privacidad y Seguridad`, tras intentar abrir el
   binario aparece **«Permitir de todas formas»**; confírmalo y vuelve a abrir.
2. O bien, desde Terminal, quita la marca de cuarentena del ejecutable (o del
   `.app`):
   `xattr -dr com.apple.quarantine /ruta/a/ai-voice-interconnector`
   (o `xattr -c` sobre el `.app` completo).
3. También funciona hacer clic derecho sobre el binario/`.app` y elegir
   **Abrir**: macOS pide confirmación una vez y luego recuerda la excepción.
4. Verifica siempre la integridad cotejando el SHA-256 del artefacto contra
   `SHA256SUMS.txt` del [Release](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases)
   antes de desbloquear (ver [docs/RELEASING.md](docs/RELEASING.md) y la guía de
   usuario en [USAGE.md](USAGE.md#el-sistema-bloquea-el-primer-arranque-binarios-sin-firmar)).
