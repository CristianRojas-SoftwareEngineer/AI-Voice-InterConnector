# Optimización de contexto en Claude Code: Engram + Codebase Memory MCP

## Introducción

Los agentes de código como **Claude Code** operan sobre una **ventana de contexto finita**. Cada
sesión arranca «en blanco»: no recuerda decisiones de sesiones anteriores y, cuando el contexto se
llena y se compacta, se pierde detalle. Además, el agente solo «ve» los archivos que abre, de modo
que carece de un mapa estructural del repositorio completo.

Este documento es un **playbook autocontenido** para instalar, configurar y usar dos herramientas de
memoria que atacan esos dos problemas desde ángulos complementarios, integradas **específicamente en
Claude Code**:

| Herramienta | Qué aporta | Tipo de memoria |
|---|---|---|
| **Engram** | Memoria **persistente** entre sesiones y compactaciones (decisiones, bugs, convenciones). | Episódica / conversacional (SQLite + FTS5). |
| **Codebase Memory MCP** (CBM) | **Grafo estructural** del código (símbolos, llamadas, dependencias) construido con tree-sitter. | Estructural / semántica del repositorio. |

**Cómo se complementan:** Engram recuerda *el porqué* (por qué se tomó una decisión, qué bug se
arregló y cómo); CBM entiende *el qué y el cómo* del código (qué función llama a qué, cómo se
relacionan los módulos). Uno es la memoria a largo plazo del agente; el otro, su comprensión
arquitectónica del proyecto. Ambas se exponen al agente como **servidores MCP** (Model Context
Protocol), el mecanismo estándar con el que Claude Code incorpora herramientas externas.

### Alcance

- **Agnóstico al proyecto:** el playbook sirve para cualquier repositorio o entorno.
- **Multiplataforma:** cada paso documenta sus variantes para **Windows**, **Linux** y **macOS**.
- **Autocontenido:** no se asume conocimiento previo de MCP, plugins ni de estas herramientas; la
  sección [1](#1-conceptos-previos-de-claude-code) cubre los conceptos mínimos necesarios.

### Convenciones del documento

- **Rutas.** `~` representa la carpeta personal del usuario:
  - Windows: `C:\Users\<tu-usuario>` (variable `%USERPROFILE%` en CMD, `$env:USERPROFILE` en
    PowerShell).
  - Linux: `/home/<tu-usuario>` (variable `$HOME`).
  - macOS: `/Users/<tu-usuario>` (variable `$HOME`).
- **Bloques de comandos.** Un bloque **sin etiqueta de sistema operativo** funciona igual en
  Windows, Linux y macOS. Cuando un paso difiere por sistema, cada variante va precedida de una
  etiqueta en negrita: **Windows (PowerShell)** para PowerShell 7 (`pwsh`) y
  **Linux / macOS (bash o zsh)** para la terminal Unix estándar.
- **Marcadores de posición.** `<ruta-al-repositorio>` debe sustituirse por la ruta real de tu
  proyecto (p. ej. `C:\proyectos\mi-repo` en Windows, `/home/usuario/proyectos/mi-repo` en Linux,
  `/Users/usuario/proyectos/mi-repo` en macOS). Análogamente, `<versión>`, `<binario>`, etc.

---

## Tabla de contenidos

- [1. Conceptos previos de Claude Code](#1-conceptos-previos-de-claude-code)
  - [1.1 Qué es un servidor MCP](#11-qué-es-un-servidor-mcp)
  - [1.2 Scopes de registro MCP](#12-scopes-de-registro-mcp)
  - [1.3 Plugins de Claude Code](#13-plugins-de-claude-code)
  - [1.4 Resolución del binario de un MCP: el problema del PATH](#14-resolución-del-binario-de-un-mcp-el-problema-del-path)
- [2. Engram — memoria persistente](#2-engram--memoria-persistente)
  - [2.1 Qué es y qué instala](#21-qué-es-y-qué-instala)
  - [2.2 Requisito previo: toolchain de Go](#22-requisito-previo-toolchain-de-go)
  - [2.3 Descarga e instalación del binario](#23-descarga-e-instalación-del-binario)
  - [2.4 Integración en Claude Code (plugin, scope global)](#24-integración-en-claude-code-plugin-scope-global)
  - [2.5 Configuración](#25-configuración)
  - [2.6 Verificación](#26-verificación)
  - [2.7 Uso](#27-uso)
  - [2.8 Desinstalación y limpieza](#28-desinstalación-y-limpieza)
- [3. Codebase Memory MCP — grafo estructural](#3-codebase-memory-mcp--grafo-estructural)
  - [3.1 Qué es y qué instala](#31-qué-es-y-qué-instala)
  - [3.2 Requisito previo: Node.js y npm](#32-requisito-previo-nodejs-y-npm)
  - [3.3 Descarga e instalación del binario](#33-descarga-e-instalación-del-binario)
  - [3.4 Integración en Claude Code (scope local por repositorio)](#34-integración-en-claude-code-scope-local-por-repositorio)
  - [3.5 Indexado del repositorio](#35-indexado-del-repositorio)
  - [3.6 Configuración](#36-configuración)
  - [3.7 Verificación](#37-verificación)
  - [3.8 Uso](#38-uso)
  - [3.9 Desinstalación y limpieza](#39-desinstalación-y-limpieza)
- [4. Verificación conjunta end-to-end](#4-verificación-conjunta-end-to-end)

---

## 1. Conceptos previos de Claude Code

### 1.1 Qué es un servidor MCP

**MCP (Model Context Protocol)** es el protocolo con el que Claude Code se comunica con herramientas
externas. Un **servidor MCP** es un programa que expone un conjunto de *tools* (funciones que el
agente puede invocar). Claude Code lanza ese programa como un subproceso y se comunica con él,
normalmente por **stdio** (entrada/salida estándar).

Tanto Engram como CBM son servidores MCP: cada uno es un binario que, invocado del modo adecuado
(`engram mcp ...` en un caso, `codebase-memory-mcp` sin argumentos en el otro), arranca en modo
servidor y ofrece sus herramientas al agente.

### 1.2 Scopes de registro MCP

Al registrar un servidor MCP en Claude Code se elige un **scope**, que determina **dónde se guarda
la configuración** y **en qué sesiones está disponible** el servidor:

| Scope | Dónde se guarda | Visibilidad | Uso típico |
|---|---|---|---|
| `local` | `~/.claude.json`, indexado por la **ruta del proyecto** | Solo en sesiones abiertas en **ese** repositorio | Herramientas específicas de un repo, privadas para ti. |
| `project` | `.mcp.json` **dentro del repo** (versionado en git) | Todos los que clonen el repo | Compartir la configuración con el equipo. |
| `user` | `~/.claude.json` (global) | **Todas** tus sesiones, en cualquier repo | Herramientas de propósito general. |

El scope se fija al registrar; para cambiarlo hay que eliminar el registro y volver a crearlo. En
este playbook:

- **Engram** se integra como **plugin**, de alcance **global (user)**: sus herramientas y hooks
  quedan disponibles en todas tus sesiones, lo deseable para una memoria transversal.
- **Codebase Memory MCP** se integra en scope **local**, ligado a un repositorio concreto, porque su
  grafo es específico de *ese* código.

### 1.3 Plugins de Claude Code

Un **plugin** es un paquete que agrupa varios artefactos (servidores MCP, *hooks*, *skills*,
comandos) y se instala de una vez desde un **marketplace** (un repositorio git que publica plugins).
Los plugins se instalan a nivel **global** y su contenido se descomprime en una caché local, con la
misma ruta relativa en los tres sistemas:

```
~/.claude/plugins/cache/<plugin>/<plugin>/<versión>/
```

Un **hook** es un script que Claude Code ejecuta automáticamente ante ciertos eventos (inicio de
sesión, fin de subagente, compactación, etc.). Engram usa hooks para **inyectar contexto** al
arrancar una sesión.

### 1.4 Resolución del binario de un MCP: el problema del PATH

Este punto es crítico: es la causa más frecuente de que un servidor MCP aparezca como **failed**.

Cuando un servidor MCP se declara con un nombre de comando a secas (p. ej. `"command": "engram"`),
Claude Code resuelve ese nombre contra el **PATH del proceso `claude`**, y ese proceso **hereda el
PATH de la terminal desde la que se lanzó** — no el PATH persistente del sistema, sino el que la
terminal tenía **en el momento de arrancar**.

Consecuencia práctica por sistema operativo:

- **Windows.** Si un binario se instala en una carpeta que se añade al PATH persistente **después**
  de haber abierto la terminal (o el IDE que la hospeda), esa terminal **no** verá el binario hasta
  que su **proceso anfitrión** se reinicie. Cerrar y reabrir pestañas no basta: hay que cerrar por
  completo el anfitrión — Windows Terminal, o el IDE entero si usas su terminal integrada — o, con
  garantías, **reiniciar el equipo**.
- **Linux y macOS.** El PATH se construye desde los archivos de arranque del shell (`~/.bashrc`,
  `~/.zshrc`, `~/.profile`, …). Si el instalador añadió una línea `export PATH=...` a uno de esos
  archivos, las terminales ya abiertas no la ven: abre una terminal nueva o ejecuta
  `source ~/.bashrc` (o `source ~/.zshrc`) antes de lanzar `claude`. Además, las aplicaciones
  lanzadas desde la interfaz gráfica (p. ej. un IDE abierto desde el Dock o el menú de aplicaciones)
  pueden heredar un PATH distinto al de tu shell; en ese caso, lanza el IDE desde la terminal o
  reinicia la sesión gráfica.

> **Regla de oro:** tras instalar un binario nuevo que vayas a exponer como MCP, abre una **terminal
> nueva** (en Windows, reinicia el anfitrión de la terminal o el equipo) antes de lanzar `claude`, y
> comprueba que el binario se resuelve **desde esa misma terminal**:
>
> **Windows (PowerShell)**
>
> ```powershell
> where.exe <binario>
> ```
>
> **Linux / macOS (bash o zsh)**
>
> ```bash
> which <binario>
> ```
>
> Si ahí no se resuelve, tampoco lo hará el MCP.

Este problema afecta especialmente a los binarios instalados con `go install`, que quedan en
`~/go/bin`, una carpeta que no siempre está en el PATH por defecto (ver
[2.3](#23-descarga-e-instalación-del-binario)).

---

## 2. Engram — memoria persistente

### 2.1 Qué es y qué instala

**Engram** (`github.com/Gentleman-Programming/engram`) es un binario escrito en Go que ofrece
memoria persistente para agentes de IA. Un mismo ejecutable reúne varias piezas:

- Un **servidor MCP** (transporte stdio) que expone herramientas como `mem_save`, `mem_search`,
  `mem_context` o `mem_session_summary`.
- Una **CLI** (`engram save`, `engram search`, `engram doctor`, …).
- Una **TUI** interactiva (`engram tui`) y una **API HTTP** (`engram serve`).

Los datos se guardan en una base **SQLite con FTS5** (búsqueda de texto completo), en la misma ruta
relativa en los tres sistemas:

```
~/.engram/engram.db
```

### 2.2 Requisito previo: toolchain de Go

La vía recomendada para obtener el binario es compilarlo con **Go** mediante `go install`: funciona
igual en los tres sistemas y evita el falso positivo de antivirus
(`Trojan:Script/Wacatac.H!ml`) que en Windows disparan a veces los binarios prearmados distribuidos
como `.zip`.

> En macOS existe además la alternativa de instalar el binario ya compilado con Homebrew (consulta
> el README del proyecto). Este playbook usa `go install` por ser el método común a los tres
> sistemas.

Comprueba si Go ya está instalado:

```bash
go version
```

Si responde algo como `go version go1.26.x <so>/amd64`, ya lo tienes y puedes saltar a
[2.3](#23-descarga-e-instalación-del-binario). Si no, instálalo:

**Windows (PowerShell)** — elige una opción:

```powershell
# Opción A: winget (gestor de paquetes de Windows)
winget install --id GoLang.Go -e

# Opción B: descargar el instalador MSI desde https://go.dev/dl/ y ejecutarlo
```

**Linux (bash o zsh)** — elige una opción:

```bash
# Opción A: gestor de paquetes de la distribución
sudo apt install golang-go        # Debian / Ubuntu
sudo dnf install golang           # Fedora
sudo pacman -S go                 # Arch

# Opción B: tarball oficial (versión más reciente que la de los repos)
# Descarga go<versión>.linux-amd64.tar.gz desde https://go.dev/dl/ y luego:
sudo rm -rf /usr/local/go
sudo tar -C /usr/local -xzf go<versión>.linux-amd64.tar.gz
echo 'export PATH=$PATH:/usr/local/go/bin' >> ~/.bashrc
```

**macOS (bash o zsh)** — elige una opción:

```bash
# Opción A: Homebrew
brew install go

# Opción B: descargar el instalador PKG desde https://go.dev/dl/ y ejecutarlo
```

Tras la instalación, Go define `GOPATH` en `~/go`; los binarios que compiles con `go install` se
colocarán en `~/go/bin`.

> Abre una **terminal nueva** (Windows: reinicia el anfitrión de la terminal) para que el PATH
> incluya `go`. Ver [1.4](#14-resolución-del-binario-de-un-mcp-el-problema-del-path).

### 2.3 Descarga e instalación del binario

Compila e instala Engram con un solo comando. Go descarga el código fuente, lo compila y deja el
ejecutable en `~/go/bin`:

```bash
go install github.com/Gentleman-Programming/engram/cmd/engram@latest
```

Esto genera `~/go/bin/engram.exe` en Windows, o `~/go/bin/engram` en Linux/macOS.

**Paso crítico — asegura que `~/go/bin` esté en el PATH persistente.** Es el paso que más fallos
causa, porque esa carpeta no siempre está en el PATH por defecto:

**Windows (PowerShell)** — el instalador de Go suele añadirla; si no, añádela así:

```powershell
$go = "$env:USERPROFILE\go\bin"
$u  = [Environment]::GetEnvironmentVariable("Path", "User")
if ($u -notlike "*$go*") {
  [Environment]::SetEnvironmentVariable("Path", "$u;$go", "User")
}
```

**Linux / macOS (bash o zsh)** — añade la línea al archivo de arranque de tu shell:

```bash
echo 'export PATH=$PATH:$HOME/go/bin' >> ~/.bashrc    # bash (habitual en Linux)
echo 'export PATH=$PATH:$HOME/go/bin' >> ~/.zshrc     # zsh (por defecto en macOS)
```

Después abre una **terminal nueva** (Windows: reinicia el anfitrión de la terminal o el equipo;
Linux/macOS: terminal nueva o `source` del archivo modificado). Ver
[1.4](#14-resolución-del-binario-de-un-mcp-el-problema-del-path).

**Verifica la instalación** desde esa terminal nueva:

```bash
engram --version        # -> engram 1.20.0 (o superior)
engram doctor           # diagnóstico read-only del entorno y la base de datos
```

Y confirma que el binario se resuelve desde el PATH:

**Windows (PowerShell)**

```powershell
where.exe engram        # debe apuntar a ~\go\bin\engram.exe
```

**Linux / macOS (bash o zsh)**

```bash
which engram            # debe apuntar a ~/go/bin/engram
```

### 2.4 Integración en Claude Code (plugin, scope global)

Engram publica un **plugin** oficial que registra automáticamente su servidor MCP y sus hooks; es la
vía recomendada de integración.

**Paso 1 — Añadir el marketplace** (el repositorio git que publica el plugin):

```bash
claude plugin marketplace add Gentleman-Programming/engram
```

**Paso 2 — Instalar el plugin:**

```bash
claude plugin install engram
```

El plugin se descomprime en `~/.claude/plugins/cache/engram/engram/<versión>/` (en el entorno donde
se validó este playbook, la versión del plugin era `0.1.1`) e instala tres tipos de artefactos:

- Un **`.mcp.json`** que declara el servidor MCP de Engram:

  ```json
  {
    "mcpServers": {
      "engram": {
        "command": "engram",
        "args": ["mcp", "--tools=agent"]
      }
    }
  }
  ```

  Observa que usa `"command": "engram"` a secas: por eso es imprescindible que el binario esté en
  el PATH que hereda `claude` (ver
  [1.4](#14-resolución-del-binario-de-un-mcp-el-problema-del-path)).

- **Hooks** que se ejecutan en eventos de sesión (`SessionStart`, `SubagentStop`, `Stop`,
  post-compactación, `user-prompt-submit`) para inyectar memoria de sesiones previas y capturar
  contexto automáticamente.

- Una **skill** de memoria.

**Paso 3 — Verificar el plugin:**

```bash
claude plugin list      # 'engram' debe aparecer habilitado
```

> **Por qué a nivel global:** al ser un plugin, Engram queda disponible en **todas** tus sesiones de
> Claude Code, no solo en un repositorio — lo deseable para una memoria transversal. Si prefieres
> acotarlo, puedes deshabilitarlo por repositorio o desinstalarlo (ver
> [2.8](#28-desinstalación-y-limpieza)).

### 2.5 Configuración

Engram funciona sin configuración adicional. Los ajustes disponibles son:

**Perfil de herramientas (`--tools`).** Controla cuántas tools expone el servidor MCP:

- `agent` — 15 tools orientadas al agente (el perfil que usa el plugin).
- `admin` — 4 tools de administración.
- `all` — las 19 tools (valor por defecto del binario).
- Se pueden combinar perfiles: `--tools=agent,admin`.

**Ubicación de los datos (`ENGRAM_DATA_DIR`).** Por defecto, `~/.engram/`. Para aislar los datos
(p. ej. en una carpeta de sandbox), define la variable de entorno antes de lanzar Claude Code:

**Windows (PowerShell)**

```powershell
$env:ENGRAM_DATA_DIR = "C:\ruta\a\mi\sandbox\.engram"   # solo la sesión actual
```

**Linux / macOS (bash o zsh)**

```bash
export ENGRAM_DATA_DIR="/ruta/a/mi/sandbox/.engram"     # solo la sesión actual
```

Para persistirla: en Windows, usa
`[Environment]::SetEnvironmentVariable("ENGRAM_DATA_DIR", "<ruta>", "User")`; en Linux/macOS, añade
la línea `export` a tu `~/.bashrc` o `~/.zshrc`.

**Proyecto por defecto (`--project` / `ENGRAM_PROJECT`).** Engram deriva el «proyecto» del
directorio de trabajo. Puedes forzarlo con `engram mcp --project <NOMBRE>` o con la variable de
entorno `ENGRAM_PROJECT`.

### 2.6 Verificación

Un ciclo `save` → `search` desde la CLI confirma que el binario y la base de datos funcionan, sin
necesidad de abrir una sesión del agente:

```bash
engram save "Prueba de humo" "Engram quedó instalado y operativo"
engram search "humo"          # debe devolver la observación recién guardada
engram doctor                 # sin errores
```

Para verificar la **integración con Claude Code**, abre una sesión en cualquier repositorio y
ejecuta el comando `/mcp`: el servidor `plugin:engram:engram` debe aparecer como **Connected**.

### 2.7 Uso

**Desde la CLI** (útil para scripts o consultas manuales):

```bash
engram save "<título>" "<contenido>" [--type decision|bugfix|pattern|...] [--project <P>]
engram search "<consulta>" [--type <T>] [--project <P>] [--limit N]
engram timeline <obs_id>      # contexto cronológico alrededor de una observación
engram tui                    # interfaz interactiva de terminal
```

**Dentro del agente (Claude Code):** con el MCP conectado, el agente dispone de las herramientas
`mem_*` y de los hooks del plugin. En la práctica:

- Al **iniciar sesión**, el hook `SessionStart` inyecta un resumen de sesiones anteriores.
- El agente **guarda** memoria con `mem_save` (decisiones, bugs, convenciones) y **busca** con
  `mem_search`.
- Al **cerrar** trabajo relevante, `mem_session_summary` persiste un resumen estructurado.
- Tras una **compactación**, `mem_context` recupera el hilo de lo que se estaba haciendo.

### 2.8 Desinstalación y limpieza

**Quitar el plugin y el marketplace:**

```bash
claude plugin uninstall engram
claude plugin marketplace remove Gentleman-Programming/engram
```

**Borrar los datos persistidos** (opcional; elimina toda la memoria):

**Windows (PowerShell)**

```powershell
Remove-Item -Recurse -Force "$env:USERPROFILE\.engram"
```

**Linux / macOS (bash o zsh)**

```bash
rm -rf ~/.engram
```

**Quitar el binario** (opcional):

**Windows (PowerShell)**

```powershell
Remove-Item "$env:USERPROFILE\go\bin\engram.exe"
```

**Linux / macOS (bash o zsh)**

```bash
rm ~/go/bin/engram
```

---

## 3. Codebase Memory MCP — grafo estructural

### 3.1 Qué es y qué instala

**Codebase Memory MCP** (CBM, paquete npm `codebase-memory-mcp`) es un servidor MCP (JSON-RPC 2.0
sobre stdio) que **indexa** un repositorio con **tree-sitter** y construye un **grafo de
conocimiento estructural**: nodos (funciones, clases, archivos, rutas) y aristas (llamadas, usos,
dependencias, similitud). Expone herramientas para consultar ese grafo, entre ellas:

`index_repository`, `search_graph`, `query_graph`, `trace_path`, `get_code_snippet`,
`get_graph_schema`, `get_architecture`, `search_code`, `list_projects`, `delete_project`,
`index_status`, `detect_changes`, `manage_adr`, `ingest_traces`.

El índice se guarda **fuera** del árbol del repositorio, en una caché por proyecto con la misma ruta
relativa en los tres sistemas:

```
~/.cache/codebase-memory-mcp/<nombre-proyecto>.db
```

> **El repositorio no se ensucia:** al indexar, CBM escribe en `~/.cache`, no en el árbol de tu
> proyecto, así que `git status` no mostrará archivos nuevos. La excepción es el flag opcional
> `--persistence`, que genera `.codebase-memory/graph.db.zst` dentro del repo para compartir el
> índice con el equipo.

### 3.2 Requisito previo: Node.js y npm

CBM se distribuye por npm, así que necesitas **Node.js** (que incluye `npm`).

Comprueba si ya está instalado:

```bash
node --version    # cualquier versión LTS reciente sirve
npm --version
```

Si ya lo tienes, salta a [3.3](#33-descarga-e-instalación-del-binario). Si no, instálalo:

**Windows (PowerShell)** — elige una opción:

```powershell
# Opción A: winget
winget install OpenJS.NodeJS.LTS

# Opción B: descargar el instalador desde https://nodejs.org/ y ejecutarlo
```

**Linux (bash o zsh)** — elige una opción:

```bash
# Opción A: gestor de paquetes de la distribución
sudo apt install nodejs npm       # Debian / Ubuntu
sudo dnf install nodejs npm       # Fedora
sudo pacman -S nodejs npm         # Arch

# Opción B: nvm (Node Version Manager) — recomendado si necesitas varias versiones
# ver https://github.com/nvm-sh/nvm
```

**macOS (bash o zsh)** — elige una opción:

```bash
# Opción A: Homebrew
brew install node

# Opción B: descargar el instalador desde https://nodejs.org/ y ejecutarlo
```

### 3.3 Descarga e instalación del binario

Instala el paquete de forma **global** (`-g`) para que el comando `codebase-memory-mcp` quede
disponible en el PATH:

```bash
npm install -g codebase-memory-mcp@latest
```

El ejecutable global de npm queda en una ubicación distinta según el sistema:

- **Windows:** `~\AppData\Roaming\npm` (el instalador de Node añade esta carpeta al PATH).
- **Linux:** según el prefijo de npm — típicamente `/usr/local/bin` o, con nvm,
  `~/.nvm/versions/node/<versión>/bin` (ambos suelen estar ya en el PATH).
- **macOS:** `/usr/local/bin` o, con Homebrew en Apple Silicon, `/opt/homebrew/bin` (ambos en el
  PATH por defecto).

> **Linux:** si `npm install -g` falla por permisos, no uses `sudo`. Configura un prefijo de usuario
> (`npm config set prefix ~/.local`) y asegúrate de que `~/.local/bin` esté en tu PATH, o usa nvm.

**Verifica la instalación** (si es la primera vez que instalas algo con `npm -g`, abre antes una
terminal nueva; ver [1.4](#14-resolución-del-binario-de-un-mcp-el-problema-del-path)):

```bash
codebase-memory-mcp --version     # -> codebase-memory-mcp 0.9.0 (o superior)
```

Y confirma que el binario se resuelve desde el PATH:

**Windows (PowerShell)**

```powershell
where.exe codebase-memory-mcp     # debe apuntar a ~\AppData\Roaming\npm\...
```

**Linux / macOS (bash o zsh)**

```bash
which codebase-memory-mcp         # debe apuntar al bin global de npm
```

### 3.4 Integración en Claude Code (scope local por repositorio)

A diferencia de Engram, CBM se integra por **repositorio**, en scope **local**, porque su grafo es
específico del código de *ese* proyecto.

Registra el servidor MCP **desde el directorio del repositorio**: el scope `local` se indexa por la
ruta del directorio de trabajo, así que este debe ser la raíz del repo objetivo:

```bash
cd "<ruta-al-repositorio>"
claude mcp add --scope local codebase-memory-mcp -- codebase-memory-mcp
```

Desglose del comando:

- `--scope local` → la configuración queda en `~/.claude.json`, ligada a esta ruta de repo.
- `codebase-memory-mcp` (primer token) → nombre con el que se registra el servidor.
- `--` → separa las opciones de `claude` del comando del servidor.
- `codebase-memory-mcp` (segundo token) → el binario que Claude lanzará (sin argumentos arranca en
  modo servidor MCP por stdio).

**Verifica el registro y su aislamiento:**

```bash
# Desde el repo objetivo: debe reportar scope 'local' y la ruta del repo
cd "<ruta-al-repositorio>" && claude mcp get codebase-memory-mcp

# Desde OTRO repo: NO debe aparecer (confirma el aislamiento por repositorio)
cd "<ruta-a-otro-repo>" && claude mcp list
```

> **Alternativa — auto-instalador de CBM.** El paquete incluye `codebase-memory-mcp install`, que
> autodetecta agentes soportados (Claude Code, Codex CLI, Gemini CLI, Zed, etc.) y escribe la
> configuración por ti. Es cómodo, pero da menos control sobre el scope; cuando quieras aislamiento
> explícito por repositorio, usa el registro manual descrito arriba.

### 3.5 Indexado del repositorio

Registrar el MCP no indexa el código automáticamente. Lanza el indexado una vez (y reláncalo cuando
el código cambie sustancialmente):

```bash
codebase-memory-mcp cli index_repository --repo-path "<ruta-al-repositorio>"
```

Modos de indexado (`--mode`): `full` (todos los archivos + aristas de similitud/semánticas),
`moderate`, `fast` (sin similitud/semánticas) y `cross-repo-intelligence` (enlaces entre
repositorios). Si se omite, se usa el modo por defecto del paquete.

**Comprueba el estado del índice:**

```bash
codebase-memory-mcp cli list_projects
```

Devuelve un JSON con los proyectos indexados: `root_path`, rama git y métricas del grafo (`nodes`,
`edges`, `size_bytes`). Como referencia, en el entorno de validación un repositorio mediano produjo
un grafo de 346 nodos y 723 aristas.

> **Sintaxis de la CLI de CBM:** `codebase-memory-mcp cli <tool> [flags]`. Las herramientas se
> invocan con flags (`--repo-path ...`); pasar JSON crudo está **deprecado**. Consulta los flags de
> cada herramienta con `codebase-memory-mcp cli <tool> --help`.

### 3.6 Configuración

Los ajustes se gestionan con `codebase-memory-mcp config <list|get|set|reset>`:

```bash
codebase-memory-mcp config list        # ver la configuración actual
```

Claves relevantes (con sus valores por defecto):

- `auto_index` (`false`) — reindexar automáticamente.
- `auto_index_limit` (`50000`) — tope de archivos para el auto-indexado.
- `auto_watch` (`true`) — vigilar cambios en el árbol.
- `ui-lang` (`auto`) — idioma de la interfaz.

**Ubicación de la caché (`CBM_CACHE_DIR`).** Por defecto, `~/.cache/codebase-memory-mcp/`. Para
aislarla (p. ej. en un sandbox), define la variable de entorno antes de lanzar el indexado o Claude
Code:

**Windows (PowerShell)**

```powershell
$env:CBM_CACHE_DIR = "C:\ruta\a\mi\sandbox\.cache-cbm"   # solo la sesión actual
```

**Linux / macOS (bash o zsh)**

```bash
export CBM_CACHE_DIR="/ruta/a/mi/sandbox/.cache-cbm"     # solo la sesión actual
```

Para persistirla, procede igual que con `ENGRAM_DATA_DIR` en [2.5](#25-configuración).

**Visualización HTTP del grafo** (opcional): `codebase-memory-mcp --ui=true --port=9749`.

### 3.7 Verificación

```bash
# El proyecto aparece indexado con métricas de grafo
codebase-memory-mcp cli list_projects

# Una consulta estructural devuelve datos reales del código
codebase-memory-mcp cli search_code --query "<un símbolo del repo>"
```

Para verificar la **integración con Claude Code**, abre una sesión **en el repositorio registrado**
y ejecuta `/mcp`: `codebase-memory-mcp` debe aparecer como **Connected**.

### 3.8 Uso

**Desde la CLI** (consultas puntuales o scripts):

```bash
codebase-memory-mcp cli search_graph   --query "<término>"
codebase-memory-mcp cli get_architecture
codebase-memory-mcp cli trace_path      --from "<símbolo A>" --to "<símbolo B>"
codebase-memory-mcp cli search_code     --query "<texto>"
```

Usa `--help` en cada herramienta para ver sus flags exactos.

**Dentro del agente (Claude Code):** con el MCP conectado en el repo, el agente puede pedirle a CBM
un mapa estructural sin abrir decenas de archivos: «¿qué llama a esta función?», «traza el camino de
A a B», «dame la arquitectura del módulo X». Esto reduce el consumo de contexto y mejora la
precisión en repositorios grandes.

### 3.9 Desinstalación y limpieza

**Quitar el registro MCP local** (desde el repo donde se registró):

```bash
cd "<ruta-al-repositorio>" && claude mcp remove --scope local codebase-memory-mcp
```

**Borrar la caché de índices** (opcional):

**Windows (PowerShell)**

```powershell
Remove-Item -Recurse -Force "$env:USERPROFILE\.cache\codebase-memory-mcp"
```

**Linux / macOS (bash o zsh)**

```bash
rm -rf ~/.cache/codebase-memory-mcp
```

**Desinstalar el paquete global** (opcional):

```bash
npm uninstall -g codebase-memory-mcp
```

Si activaste `--persistence` al indexar, revisa y elimina también `.codebase-memory/` dentro del
repo.

---

## 4. Verificación conjunta end-to-end

Lista de comprobación final tras integrar ambas herramientas en un repositorio:

1. **Binarios resolubles** en la terminal desde la que lanzarás Claude Code:

   **Windows (PowerShell)**

   ```powershell
   where.exe engram; where.exe codebase-memory-mcp
   ```

   **Linux / macOS (bash o zsh)**

   ```bash
   which engram && which codebase-memory-mcp
   ```

2. **Engram operativo** (ciclo CLI):

   ```bash
   engram save "check" "ok" && engram search "check" && engram doctor
   ```

3. **CBM indexado y aislado por repo:**

   ```bash
   codebase-memory-mcp cli list_projects          # el repo aparece con nodos/aristas
   cd "<ruta-al-repositorio>" && claude mcp get codebase-memory-mcp   # scope local
   ```

4. **Ambos conectados en el agente.** Abre la sesión en el repositorio objetivo y ejecuta `/mcp`:

   ```bash
   cd "<ruta-al-repositorio>" && claude
   # dentro de Claude Code:
   /mcp
   # esperado:
   #   plugin:engram:engram        ✔ Connected
   #   codebase-memory-mcp         ✔ Connected
   ```

Si `/mcp` muestra alguno como **failed**, revisa primero la
[sección 1.4](#14-resolución-del-binario-de-un-mcp-el-problema-del-path): comprueba que el binario
se resuelve (`where.exe` en Windows, `which` en Linux/macOS) en la terminal desde la que abriste
Claude Code y, si no se resuelve, abre una terminal nueva (Windows: reinicia el anfitrión de la
terminal o el equipo) y vuelve a intentarlo.
