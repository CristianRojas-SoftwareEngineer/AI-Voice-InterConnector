# Ayudas del arnes local para install.tests.ps1 (solo pruebas).
#
# El servidor falso (support/Serve.ps1, HTTP en 127.0.0.1 con puerto efimero)
# sirve el asset de Windows y un SHA256SUMS.txt coherente o corrupto. La base
# se fija con AVI_DOWNLOAD_BASE_URL y las cuatro raices se reubican a
# temporales. La version de prueba es 9.9.9: lo inexistente (8.8.8) falla con
# error de red, lo que demuestra que resolucion se eligio.
#
# El falso ai-voice-interconnector.exe (doble de `self install`, compilado una
# vez por ejecucion con csc.exe) registra sus argumentos en AVI_FAKE_LOG,
# con AVI_FAKE_MODE=noexec no arranca (binario incompatible) y, cuando se le
# pide con AVI_FAKE_PATH_SUBKEY, emula el contrato PATH de `self install`
# contra esa subclave de prueba: sin --no-modify-path anade el directorio de
# programa al valor Path, con --no-modify-path no toca el registro.

$script:HarnessVersion = "9.9.9"
$script:SavedEnv = @{}
$script:ChildEngine = ""

# Motor del hijo por defecto: powershell.exe (5.1, el del executor CI) si existe.
function Get-ChildEngine {
    if ([string]::IsNullOrEmpty($script:ChildEngine)) {
        $desktop = Get-Command powershell.exe -ErrorAction SilentlyContinue
        if ($null -ne $desktop) { $script:ChildEngine = $desktop.Source }
        else { $script:ChildEngine = (Get-Command pwsh -ErrorAction SilentlyContinue).Source }
    }
    return $script:ChildEngine
}

# El hijo hereda el PSModulePath del anfitrion: bajo PowerShell 7 el directorio
# de PS7 precede al del motor hijo y su `Microsoft.PowerShell.Utility` (.NET
# Core) hace sombra al de Windows PowerShell (la autocarga falla). El
# directorio del motor hijo va primero, se mueva o se anada. -Engine vacio
# usa el motor por defecto (Get-ChildEngine).
function Get-ChildModulePath {
    param([string]$Current, [string]$Engine = "")
    if ([string]::IsNullOrEmpty($Engine)) { $Engine = Get-ChildEngine }
    if ($Engine -match "powershell\.exe$") {
        $needDir = Join-Path ([Environment]::GetFolderPath("System")) "WindowsPowerShell\v1.0\Modules"
    } else {
        $needDir = Join-Path (Split-Path $Engine -Parent) "Modules"
    }
    $rest = @($Current -split ";" | Where-Object { $_ -ne "" -and $_ -ne $needDir })
    return (@($needDir) + $rest) -join ";"
}

# Aplica las variables del hashtable (valor $null = ausente) guardando las previas.
function Enter-HarnessEnv {
    param([hashtable]$Vars)
    foreach ($key in $Vars.Keys) {
        $script:SavedEnv[$key] = [Environment]::GetEnvironmentVariable($key)
        [Environment]::SetEnvironmentVariable($key, $Vars[$key])
    }
}

# Restaura lo guardado por Enter-HarnessEnv.
function Exit-HarnessEnv {
    foreach ($key in @($script:SavedEnv.Keys)) {
        [Environment]::SetEnvironmentVariable($key, $script:SavedEnv[$key])
    }
    $script:SavedEnv = @{}
}

# Compila el falso ejecutable una vez (csc.exe de .NET Framework, presente en
# el executor Windows y en cualquier Windows con PowerShell 5.1 o 7; Add-Type
# no puede emitir ensamblados ConsoleApplication en PowerShell 7).
function New-FakeExe {
    param([string]$Path)
    $source = @'
using System;
public static class AviFakeBootstrap {
    public static int Main(string[] args) {
        string mode = System.Environment.GetEnvironmentVariable("AVI_FAKE_MODE") ?? "ok";
        if (args.Length == 1 && args[0] == "--version") {
            if (mode == "noexec") return 3;
            System.Console.Out.WriteLine("ai-voice-interconnector 9.9.9");
            return 0;
        }
        if (mode == "noexec") return 3;
        string log = System.Environment.GetEnvironmentVariable("AVI_FAKE_LOG");
        if (!string.IsNullOrEmpty(log)) {
            System.IO.File.AppendAllText(log, string.Join(" ", args) + System.Environment.NewLine);
        }
        string subkey = System.Environment.GetEnvironmentVariable("AVI_FAKE_PATH_SUBKEY");
        bool noModify = System.Array.Exists(args, delegate(string a) { return a == "--no-modify-path"; });
        if (!string.IsNullOrEmpty(subkey) && !noModify) {
            string programDir = System.Environment.GetEnvironmentVariable("AVI_INSTALL_DIR") ?? "";
            using (Microsoft.Win32.RegistryKey key = Microsoft.Win32.Registry.CurrentUser.CreateSubKey(subkey)) {
                string current = (key.GetValue("Path") as string) ?? "";
                string[] parts = current.Split(new char[] { ';' }, System.StringSplitOptions.RemoveEmptyEntries);
                if (System.Array.IndexOf(parts, programDir) < 0) {
                    key.SetValue("Path", string.IsNullOrEmpty(current) ? programDir : current + ";" + programDir);
                }
            }
        }
        return 0;
    }
}
'@
    $csc = Join-Path ([Environment]::GetFolderPath("Windows")) "Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { throw "no se encontro csc.exe en $csc" }
    $csFile = [IO.Path]::ChangeExtension($Path, ".cs")
    Set-Content -Path $csFile -Value $source -Encoding ASCII
    & $csc /nologo /target:exe /out:"$Path" "$csFile"
    if ($LASTEXITCODE -ne 0) { throw "csc fallo con codigo $LASTEXITCODE" }
    Remove-Item -Force $csFile -ErrorAction SilentlyContinue
}# Empaqueta el asset de Windows (zip con el falso exe) en <Dir>/v<ver>/.
function New-HarnessAsset {
    param([string]$Dir, [string]$FakeExe)
    $version = $script:HarnessVersion
    $vdir = Join-Path $Dir "v$version"
    New-Item -ItemType Directory -Force -Path $vdir | Out-Null
    $stage = Join-Path $Dir "stage-win"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item $FakeExe (Join-Path $stage "ai-voice-interconnector.exe")
    $zip = Join-Path $vdir "ai-voice-interconnector-$version-x86_64-windows.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
    Remove-Item -Recurse -Force $stage
    return $zip
}

# Escribe SHA256SUMS.txt: coherente, corrupto o sin linea para el asset.
function Write-HarnessChecksum {
    param([string]$Dir, [string]$Mode = "ok")
    $version = $script:HarnessVersion
    $vdir = Join-Path $Dir "v$version"
    $zipName = "ai-voice-interconnector-$version-x86_64-windows.zip"
    $zip = Join-Path $vdir $zipName
    $sums = Join-Path $vdir "SHA256SUMS.txt"
    if ($Mode -eq "missing") {
        Set-Content -Path $sums -Value "# sin linea para el asset" -NoNewline
        return
    }
    $hash = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Mode -eq "corrupt") { $hash = ("0" * 63) + "f" }
    Set-Content -Path $sums -Value "$hash  $zipName" -NoNewline
}

# Arranca el servidor sobre <Root>; devuelve @{ Process; Port; RequestLog }.
function Start-HarnessServer {
    param([string]$Root, [string]$WorkDir, [string]$ServeScript)
    $portFile = Join-Path $WorkDir "port"
    $requestLog = Join-Path $WorkDir "requests.log"
    if (Test-Path $portFile) { Remove-Item -Force $portFile }
    Set-Content -Path $requestLog -Value "" -NoNewline
    $engine = Get-ChildEngine
    $proc = Start-Process -FilePath $engine -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $ServeScript, "-Root", $Root, "-PortFile", $portFile, "-RequestLog", $requestLog) -NoNewWindow -PassThru
    $port = ""
    for ($i = 0; $i -lt 100; $i++) {
        if (Test-Path $portFile) {
            # El hijo puede estar escribiendo el fichero con bloqueo exclusivo:
            # una lectura fallida es transitoria y se reintenta, no aborta.
            try { $port = (Get-Content -Raw $portFile -ErrorAction Stop) } catch { $port = "" }
            if ($port -match "^\d+$") {
                try {
                    $tcp = New-Object Net.Sockets.TcpClient
                    $task = $tcp.BeginConnect("127.0.0.1", [int]$port, $null, $null)
                    if ($task.AsyncWaitHandle.WaitOne(500)) { $tcp.EndConnect($task); $tcp.Close(); break }
                    $tcp.Close()
                } catch { Write-Verbose "puerto aun no listo, se reintenta" }
            }
        }
        Start-Sleep -Milliseconds 200
    }
    if ($port -notmatch "^\d+$") {
        Stop-Process -InputObject $proc -Force -ErrorAction SilentlyContinue
        throw "el servidor falso no arranco"
    }
    return @{ Process = $proc; Port = $port; RequestLog = $requestLog }
}

# Detiene el servidor de Start-HarnessServer. Tolera que ya haya terminado:
# el AfterEach corre tambien cuando el arranque fallo y el objeto es de la
# prueba anterior, ya detenida.
function Stop-HarnessServer {
    param($Server)
    if ($null -ne $Server) {
        Stop-Process -InputObject $Server.Process -Force -ErrorAction SilentlyContinue
        try { $Server.Process.WaitForExit(5000) } catch { Write-Verbose "el servidor ya habia terminado" }
    }
}

# Ejecuta el bootstrap en un hijo: por fichero (-File + argumentos) o por
# stdin (-StdinText alimenta `-Command -`; las pruebas de tuberia pasan por
# ahi la linea `irm <url> | iex`). -Engine elige el motor del hijo (vacio =
# Get-ChildEngine). Devuelve ExitCode/Stdout/Stderr.
#
# -ExtraEnv vive en el anfitrion (heredado por el hijo y restaurado despues).
# -ChildEnv vive SOLO en el hijo (prefijo `set` en la linea de cmd): las
# variables de sistema como PROCESSOR_ARCHITECTURE no deben tocarse en el
# anfitrion -sobrescribirlas rompe su herencia a los nietos en PowerShell 7
# (comprobado: el hijo las ve vacias aunque el padre lea el valor restaurado).
# Los valores de -ChildEnv no admiten comillas dobles.
#
# El hijo cuelga de cmd.exe con redireccion a ficheros, a proposito:
# `Start-Process -PassThru` devuelve en Windows PowerShell 5.1 un proceso cuyo
# ExitCode se lee vacio (comprobado), mientras que Process::Start lo expone
# bien; y el error del bootstrap vuelca todo el bloque `& {}` (~12 KB), que
# interbloquearia la lectura secuencial de los tubos. Con ficheros no hay
# limite ni interbloqueo posible.
function Invoke-ChildBootstrap {
    param([string]$Bootstrap = "", [string[]]$Arguments = @(), [hashtable]$ExtraEnv = @{}, [hashtable]$ChildEnv = @{}, [string]$StdinText, [string]$Engine = "")
    if ([string]::IsNullOrEmpty($Engine)) { $Engine = Get-ChildEngine }
    Enter-HarnessEnv $ExtraEnv
    $origModulePath = [Environment]::GetEnvironmentVariable("PSModulePath")
    [Environment]::SetEnvironmentVariable("PSModulePath", (Get-ChildModulePath $origModulePath -Engine $Engine))
    try {
        $viaStdin = $PSBoundParameters.ContainsKey("StdinText")
        $ioDir = Join-Path ([IO.Path]::GetTempPath()) ("avi-io-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Force -Path $ioDir | Out-Null
        try {
            $stdoutFile = Join-Path $ioDir "stdout.txt"
            $stderrFile = Join-Path $ioDir "stderr.txt"
            $setPrefix = ""
            foreach ($key in $ChildEnv.Keys) {
                $setPrefix += "set `"$key=$($ChildEnv[$key])`" && "
            }
            $parts = @("`"" + $Engine + "`"", "-NoProfile", "-ExecutionPolicy", "Bypass")
            if ($viaStdin) {
                $parts += @("-Command", "-")
            } else {
                $parts += @("-File", "`"$Bootstrap`"")
                foreach ($arg in $Arguments) { $parts += @("`"$arg`"") }
            }
            $cmdLine = ($parts -join " ") + " > `"$stdoutFile`" 2> `"$stderrFile`""
            if ($viaStdin) {
                $stdinFile = Join-Path $ioDir "stdin.txt"
                [IO.File]::WriteAllBytes($stdinFile, [Text.Encoding]::UTF8.GetBytes($StdinText))
                $cmdLine += " < `"$stdinFile`""
            }
            $psi = New-Object System.Diagnostics.ProcessStartInfo
            $psi.FileName = "cmd.exe"
            $psi.Arguments = '/c "' + $setPrefix + $cmdLine + '"'
            $psi.UseShellExecute = $false
            $psi.CreateNoWindow = $true
            $child = [System.Diagnostics.Process]::Start($psi)
            if (-not $child.WaitForExit(120000)) {
                try { $child.Kill() } catch { Write-Verbose "el hijo ya habia terminado" }
                throw "el hijo del bootstrap no termino en 120 s"
            }
            $stdout = ""
            $stderr = ""
            if (Test-Path $stdoutFile) { $stdout = (Get-Content -Raw $stdoutFile) }
            if (Test-Path $stderrFile) { $stderr = (Get-Content -Raw $stderrFile) }
            if ($null -eq $stdout) { $stdout = "" }
            if ($null -eq $stderr) { $stderr = "" }
            return @{ ExitCode = $child.ExitCode; Stdout = $stdout; Stderr = $stderr }
        } finally {
            [Environment]::SetEnvironmentVariable("PSModulePath", $origModulePath)
            Remove-Item -Recurse -Force $ioDir -ErrorAction SilentlyContinue
        }
    } finally {
        Exit-HarnessEnv
    }
}
