<#
.SYNOPSIS
Bootstrap de primera instalacion de ai-voice-interconnector para Windows.
.DESCRIPTION
Detecta el target, resuelve la version, descarga por HTTPS, verifica el hash
con comparacion exacta, comprueba que el binario arranca y delega en
`self install`. Sin -Check: con el binario instalado se usa
`self update --check`. Toda opcion tiene variable de entorno equivalente,
porque `irm | iex` no admite parametros. Su unico efecto sobre la sesion es
anadir el directorio de programa al PATH de esa sesion.
.EXAMPLE
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
.EXAMPLE
$env:AVI_NO_SETUP = "1"; irm <url> | iex
#>
# Sin bloque param() en el nivel del script: bajo `irm | iex` sus variables
# quedarian en la sesion del usuario. Los parametros viven en el bloque de
# ambito propio y se enlazan con @args (como archivo admiten -Version y
# conmutadores; bajo `irm | iex` mandan las variables AVI_*).
& {
    param(
        [string]$Version = "",
        [switch]$NoSetup,
        [switch]$NoModifyPath,
        [switch]$Yes,
        [string]$InvocationName = ""
    )

    $ErrorActionPreference = "Stop"
    $ProgressPreference = "SilentlyContinue"

    # Este archivo es ASCII puro y sin BOM: `irm` entrega el cuerpo sin charset
    # (ISO-8859-1 en PowerShell 5.1, UTF-8 con U+FEFF conservado en 7), asi que
    # un BOM rompe el parseo y un byte no ASCII corrompe los mensajes. Las
    # letras con tilde de los mensajes se componen al ejecutarse.
    $a = [char]0x00E1; $i = [char]0x00ED; $o = [char]0x00F3; $u = [char]0x00FA

    $Repo = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector"
    $App = "ai-voice-interconnector"

    # Version estampada al publicar el release (la sustituye la automatizacion de
    # publicacion, una sola vez).
    $StampedVersion = "__AVI_STAMPED_VERSION__"

    function Write-BootstrapLog {
        param([string]$Message)
        Write-Host $Message
    }

    function Test-BootstrapFlag {
        param([string]$Name)
        $value = [Environment]::GetEnvironmentVariable($Name)
        return ($value -eq "1" -or $value -eq "true" -or $value -eq "yes")
    }

    function Test-BootstrapVersion {
        # Exactamente X.Y.Z (tres componentes numericos). `\z` y no `$`: este
        # ultimo aceptaria un salto de linea final.
        param([string]$Value)
        return ($Value -match '^[0-9]+\.[0-9]+\.[0-9]+\z')
    }

    function Resolve-BootstrapLatest {
        # Ultima estable sin API REST: se sigue la redireccion de releases/latest.
        # La URL final vive en sitios distintos segun la version (5.1: respuesta
        # HTTP con ResponseUri; 7+: mensaje con RequestMessage.RequestUri).
        $response = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -UseBasicParsing -MaximumRedirection 10 -ErrorAction SilentlyContinue
        if ($null -ne $response) {
            $final = ""
            if ($PSVersionTable.PSVersion.Major -ge 6) {
                $final = [string]$response.BaseResponse.RequestMessage.RequestUri
            } else {
                $final = [string]$response.BaseResponse.ResponseUri.AbsoluteUri
            }
            $tag = $final.Substring($final.LastIndexOf("/") + 1).TrimStart("v")
            if (Test-BootstrapVersion $tag) { return $tag }
        }
        # Respaldo con la API (objeto, sin parsear JSON a mano).
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing -Headers @{ "User-Agent" = "ai-voice-interconnector-bootstrap" } -ErrorAction SilentlyContinue
        if ($null -ne $release) {
            $tag = ([string]$release.tag_name).TrimStart("v")
            if (Test-BootstrapVersion $tag) { return $tag }
        }
        return ""
    }

    function Resolve-BootstrapVersion {
        param([string]$Wanted, [string]$Stamped)
        $version = $Wanted.TrimStart("v")
        $isMarker = [string]::IsNullOrWhiteSpace($Stamped) -or $Stamped.StartsWith("__AVI_")
        if ([string]::IsNullOrWhiteSpace($version) -and (-not $isMarker)) { $version = $Stamped }
        if ([string]::IsNullOrWhiteSpace($version)) {
            $version = Resolve-BootstrapLatest
            if ([string]::IsNullOrWhiteSpace($version)) {
                throw "ERROR [network_error]: no se pudo resolver la ${u}ltima versi${o}n (sin red o sin GitHub). Fija una con -Version X.Y.Z o AVI_VERSION."
            }
        }
        if (-not (Test-BootstrapVersion $version)) {
            throw "ERROR [usage_error]: versi${o}n inv${a}lida: '$version' (se espera X.Y.Z)."
        }
        return $version
    }

    function Resolve-BootstrapTarget {
        # Arquitectura nativa del SO, no la del proceso (WOW64 o emulacion).
        $osArch = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITEW6432")
        if ([string]::IsNullOrWhiteSpace($osArch)) { $osArch = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE") }
        if ([string]::IsNullOrWhiteSpace($osArch)) {
            throw "ERROR [unsupported_platform]: no se pudo detectar la arquitectura del sistema. Alternativa: compila desde la fuente (docs/BUILD.md)."
        }
        if ($osArch -eq "AMD64") { return @{ Target = "x86_64-pc-windows-msvc"; Arch = "x86_64" } }
        throw "ERROR [unsupported_platform]: arquitectura no soportada: $osArch (solo Windows x64; ARM64 no soportado). Alternativa: compila desde la fuente (docs/BUILD.md)."
    }

    function Invoke-BootstrapInstall {
        # TLS 1.2; la barra de progreso ya esta desactivada arriba.
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        } catch {
            Write-BootstrapLog "AVISO: no se pudo forzar TLS 1.2: $_"
        }

        # Con sesion elevada se avisa: la instalacion es per-user, sin tocar HKLM.
        $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
        if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
            Write-BootstrapLog "AVISO: esta sesi${o}n est${a} elevada, pero la instalaci${o}n es per-user y se har${a} en el perfil de la cuenta actual."
        }

        $wanted = $Version
        if ([string]::IsNullOrWhiteSpace($wanted)) { $wanted = [Environment]::GetEnvironmentVariable("AVI_VERSION") }
        $noSetup = $NoSetup -or (Test-BootstrapFlag "AVI_NO_SETUP")
        $noModifyPath = $NoModifyPath -or (Test-BootstrapFlag "AVI_NO_MODIFY_PATH")
        $yes = $Yes -or (Test-BootstrapFlag "AVI_YES")

        $targetInfo = Resolve-BootstrapTarget
        $target = $targetInfo.Target
        $version = Resolve-BootstrapVersion -Wanted $wanted -Stamped $StampedVersion

        $programDir = [Environment]::GetEnvironmentVariable("AVI_INSTALL_DIR")
        if ([string]::IsNullOrWhiteSpace($programDir)) {
            $programDir = Join-Path ([Environment]::GetEnvironmentVariable("LOCALAPPDATA")) "Programs\$App"
        }
        $parentDir = Split-Path $programDir -Parent
        if (-not (Test-Path $parentDir)) { New-Item -ItemType Directory -Path $parentDir -Force | Out-Null }
        $staging = Join-Path $parentDir (".ai-voice-interconnector-staging-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $staging -Force | Out-Null
        try {
            $acl = Get-Acl -Path $staging
            $acl.SetAccessRuleProtection($true, $false)
            $identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
            $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($identity, "FullControl", "ContainerInherit,ObjectInherit", "None", "Allow")
            $acl.AddAccessRule($rule)
            Set-Acl -Path $staging -AclObject $acl
        } catch {
            Write-BootstrapLog "AVISO: no se pudieron restringir los permisos del staging: $_"
        }

        $code = 1
        try {
            $base = [Environment]::GetEnvironmentVariable("AVI_DOWNLOAD_BASE_URL")
            if ([string]::IsNullOrWhiteSpace($base)) { $base = "https://github.com/$Repo/releases/download" }
            $archiveName = "$App-$version-x86_64-windows.zip"
            $archivePath = Join-Path $staging $archiveName
            $sumsPath = Join-Path $staging "SHA256SUMS.txt"
            Write-BootstrapLog "Instalando $App $version ($target)..."
            try {
                Invoke-WebRequest -Uri "$base/v$version/$archiveName" -OutFile $archivePath -UseBasicParsing
            } catch {
                throw "ERROR [network_error]: descarga fallida: $archiveName. $_"
            }
            try {
                Invoke-WebRequest -Uri "$base/v$version/SHA256SUMS.txt" -OutFile $sumsPath -UseBasicParsing
            } catch {
                throw "ERROR [network_error]: descarga fallida: SHA256SUMS.txt. $_"
            }

            # Verificacion exacta antes de extraer: el nombre coincide cadena a cadena.
            $expected = $null
            foreach ($line in (Get-Content -Path $sumsPath)) {
                $parts = $line -split '\s+', 3
                if ($parts.Count -ge 2 -and $parts[1].TrimStart("*") -ceq $archiveName) { $expected = $parts[0].ToLowerInvariant(); break }
            }
            if ([string]::IsNullOrWhiteSpace($expected)) { throw "ERROR [checksum_mismatch]: SHA256SUMS.txt no contiene ninguna l${i}nea para $archiveName; instalaci${o}n abortada." }
            $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -cne $expected) { throw "ERROR [checksum_mismatch]: el checksum de $archiveName no coincide con SHA256SUMS.txt; instalaci${o}n abortada." }
            Write-BootstrapLog "Checksum verificado: $archiveName"

            Expand-Archive -Path $archivePath -DestinationPath $staging -Force
            $exe = Join-Path $staging "$App.exe"
            if (-not (Test-Path $exe)) { throw "ERROR [bundle_invalid]: el archivo no contiene el ejecutable esperado: $App.exe." }
            $incompatible = "ERROR [binary_incompatible]: el binario descargado ($target) no arranca en este sistema. Alternativa: compila desde la fuente siguiendo docs/BUILD.md. La instalaci${o}n existente no se ha modificado."
            try {
                & $exe --version | Out-Null
            } catch {
                throw $incompatible
            }
            if ($LASTEXITCODE -ne 0) { throw $incompatible }

            $installArgs = @("self", "install")
            if ($noSetup) { $installArgs += "--no-setup" }
            if ($noModifyPath) { $installArgs += "--no-modify-path" }
            if ($yes) { $installArgs += "--yes" }
            & $exe @installArgs
            $code = $LASTEXITCODE
            if ($code -eq 0) {
                # Unico efecto sobre la sesion: el programa en el PATH en curso.
                if (($env:Path -split ';') -notcontains $programDir) { $env:Path = "$env:Path;$programDir" }
            }
        } finally {
            Remove-Item -Path $staging -Recurse -Force -ErrorAction SilentlyContinue
        }
        return $code
    }

    if ($InvocationName -eq ".") { return }
    $fromFile = -not [string]::IsNullOrEmpty($InvocationName)
    try {
        $resultCode = Invoke-BootstrapInstall
    } catch {
        # No terminante a proposito: bajo `irm | iex` la sesion sigue viva.
        Write-Error $_ -ErrorAction Continue
        $resultCode = 1
    }
    # Bajo `irm | iex` nunca se usa `exit`: un error no cierra la consola.
    if ($fromFile) { exit $resultCode }
} -InvocationName $MyInvocation.InvocationName @args
