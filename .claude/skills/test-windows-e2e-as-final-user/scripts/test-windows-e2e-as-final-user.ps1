[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Run = $null
$script:Results = [System.Collections.Generic.List[object]]::new()
$script:Bin = $null
$script:ProgramDir = $null
$script:Root = $null
$script:Version = $null
$script:DaemonOwned = $false
$script:DaemonPid = $null
$script:Devices = @()
$script:Reference = $null
$script:WavShortDefault = $null
$script:WavShortClone = $null
$script:WerResults = [System.Collections.Generic.List[object]]::new()
$script:WavRetained = $null
$script:DenoiseOutput = @()
$script:DenoiseOutputByPreset = @{}
$script:DataDir = $null
$script:CacheDir = $null
$script:ModelsDir = $null
$script:CleanupTargetSpecs = @()
$script:UninstallTargetSpecs = @()
$script:ExpectedWindowsSha256 = $null
$script:InputAvailable = $false
$script:OutputAvailable = $false
$script:SubcaseResults = [System.Collections.Generic.List[object]]::new()
$script:DaemonStartedIdentities = [System.Collections.Generic.HashSet[int]]::new()
$script:TotalStopwatch = $null
$script:Release = $null
# Fuente única de IDs y títulos de fase, en orden de ejecución.
$script:Phases = @(
    [pscustomobject]@{Id='01-release';Title='Resolución del release publicado'}
    [pscustomobject]@{Id='02-source-gate';Title='Gate Rust del código fuente'}
    [pscustomobject]@{Id='03-cleanup';Title='Inventario y limpieza inicial'}
    [pscustomobject]@{Id='04-bootstrap';Title='Bootstrap publicado'}
    [pscustomobject]@{Id='05-setup';Title='Setup de modelos'}
    [pscustomobject]@{Id='06-daemon';Title='Daemon'}
    [pscustomobject]@{Id='07-voices';Title='Voces'}
    [pscustomobject]@{Id='08-synthesis';Title='Síntesis y WER'}
    [pscustomobject]@{Id='09-store';Title='Store de locuciones y voces'}
    [pscustomobject]@{Id='10-stt';Title='Transcripción'}
    [pscustomobject]@{Id='11-translate';Title='Traducción'}
    [pscustomobject]@{Id='12-dub';Title='Doblaje'}
    [pscustomobject]@{Id='13-denoise';Title='Denoise'}
    [pscustomobject]@{Id='14-final-state';Title='Estado final'}
)

function Format-Duration {
    param([long]$Milliseconds)
    $ts = [TimeSpan]::FromMilliseconds($Milliseconds)
    if ($ts.TotalMinutes -ge 1) {
        return ('{0}m {1:d2}s ({2:N0} ms)' -f [int][Math]::Floor($ts.TotalMinutes), $ts.Seconds, $Milliseconds)
    } else {
        return ('{0:N2}s ({1:N0} ms)' -f $ts.TotalSeconds, $Milliseconds)
    }
}

function Write-Result {
    param([Parameter(Mandatory)][string]$Status, [Parameter(Mandatory)][string]$Message)
    Write-Host ("{0} {1}" -f $Status, $Message)
}

function Write-TimingSummary {
    param([long]$TotalMs)
    $totalFormatted = Format-Duration $TotalMs
    Write-Host ""
    Write-Host ("=" * 115) -ForegroundColor Cyan
    Write-Host (" RESUMEN DE TIEMPOS DE EJECUCIÓN (Total: {0})" -f $totalFormatted) -ForegroundColor Cyan
    Write-Host ("=" * 115) -ForegroundColor Cyan
    Write-Host (" {0,-16} | {1,-32} | {2,-8} | {3,-20} | {4}" -f "Fase", "Título", "Estado", "Duración", "Detalle") -ForegroundColor Cyan
    Write-Host ("-" * 115) -ForegroundColor Cyan
    $summarySteps = @('preflight') + @($script:Phases.Id) + @('run')
    $phaseTitles = @{}; foreach ($phase in $script:Phases) { $phaseTitles[$phase.Id] = $phase.Title }
    foreach ($res in $script:Results) {
        if ($res.step -in $summarySteps) {
            $durStr = if ($res.duration_formatted) { [string]$res.duration_formatted } else { "-" }
            $color = switch ($res.status) {
                'PASS' { 'Green' }
                'FAIL' { 'Red' }
                'BLOCKED' { 'Yellow' }
                'SKIP' { 'DarkGray' }
                'NOT_RUN' { 'DarkYellow' }
                default { 'White' }
            }
            $title = if ($phaseTitles.ContainsKey($res.step)) { $phaseTitles[$res.step] } else { '-' }
            Write-Host (" {0,-16} | {1,-32} | {2,-8} | {3,-20} | {4}" -f $res.step, $title, $res.status, $durStr, $res.detail) -ForegroundColor $color
        }
    }
    Write-Host ("=" * 115) -ForegroundColor Cyan
    Write-Host ""
}

function Get-SourceVersion {
    param([Parameter(Mandatory)][string]$MainFile)
    $match = Select-String -LiteralPath $MainFile -Pattern '^const VERSION: &str = "([0-9]+\.[0-9]+\.[0-9]+)";' | Select-Object -First 1
    if ($null -eq $match) { throw 'No se pudo resolver VERSION en src/main.rs.' }
    return $match.Matches[0].Groups[1].Value
}

function Assert-True {
    param([Parameter(Mandatory)]$Condition, [Parameter(Mandatory)][string]$Message)
    if (!$Condition) { throw $Message }
}

function Initialize-Modules {
    param([Parameter(Mandatory)][System.Management.Automation.Language.Ast]$Ast)
    # Solo .NET y cmdlets de Microsoft.PowerShell.Core: este código corre antes de saber si los demás módulos estándar existen.
    $defined = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($function in $Ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true)) { [void]$defined.Add($function.Name) }
    $used = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($command in $Ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.CommandAst] }, $true)) {
        $name = $command.GetCommandName()
        if ($name -and $name -match '^[A-Za-z]+-[A-Za-z]+$' -and -not $defined.Contains($name)) { [void]$used.Add($name) }
    }
    $missing = @($used | Where-Object { -not (Get-Command -Name $_ -ErrorAction SilentlyContinue) })
    if (-not $missing.Count) { return $false }
    # Un PSModulePath heredado de PowerShell 7 sombrea los módulos de Windows PowerShell 5.1: se importan por ruta absoluta bajo $PSHOME sin modificar PSModulePath.
    foreach ($module in @('Microsoft.PowerShell.Utility', 'Microsoft.PowerShell.Management', 'Microsoft.PowerShell.Security', 'Microsoft.PowerShell.Archive')) {
        Import-Module ([IO.Path]::Combine($PSHOME, 'Modules', $module)) -ErrorAction Stop
    }
    $missing = @($used | Where-Object { -not (Get-Command -Name $_ -ErrorAction SilentlyContinue) })
    if ($missing.Count) { throw "BLOCKED: faltan cmdlets estándar de PowerShell: $($missing -join ', ')" }
    return $true
}

function Initialize-Run {
    $script:TotalStopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    $preflightWatch = [System.Diagnostics.Stopwatch]::StartNew()
    # El parseo AST es solo .NET y debe ir primero: la comprobación de módulos precede a cualquier cmdlet no verificado.
    $astTokens = $null
    $astErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($PSCommandPath, [ref]$astTokens, [ref]$astErrors)
    if ($astErrors.Count) { throw 'El parser AST detectó errores; no se ejecutará ninguna fase.' }
    $modulesRepaired = Initialize-Modules -Ast $ast
    if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) { throw 'Se requiere Windows x64.' }
    if ($PSVersionTable.PSVersion -lt [version]'5.1') { throw 'Se requiere PowerShell 5.1 o posterior.' }
    if ([string]::IsNullOrWhiteSpace($env:TEMP) -or [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { throw 'Faltan TEMP o LOCALAPPDATA.' }

    $script:Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..\..\..')).Path
    if($env:AVI_NO_SETUP){throw 'BLOCKED: AVI_NO_SETUP ya está definido externamente; no se continúa con entorno de bootstrap ambiguo.'}
    foreach($variable in @('AVI_INSTALL_DIR','AVI_BIN_DIR','AVI_DATA_DIR','AVI_CACHE_DIR','HF_HUB_CACHE','HF_HOME','AVI_DAEMON_PORT','AVI_DOWNLOAD_BASE_URL','AVI_VERSION','AVI_YES','AVI_NO_MODIFY_PATH','QWEN3_TTS_BIN','QWEN3_TTS_MODEL_DIR','QWEN3_TTS_BASE_MODEL_DIR','AVI_ALLOW_CWD_VENDOR')){if([Environment]::GetEnvironmentVariable($variable)){throw "BLOCKED: $variable altera versión, rutas, motor, confirmación automática o transporte de esta corrida."}}
    foreach ($relative in @('SKILL.md', 'src\main.rs', 'Cargo.toml', 'tests\golden\cli_version.json', 'assets\audios\Speech Sampler.wav', 'crates\avi-stt\tests\assets\parakeet_sample_16k.wav', 'packaging\bootstrap\install.ps1')) {
        $path = if ($relative -eq 'SKILL.md') { Join-Path $PSScriptRoot "..\$relative" } else { Join-Path $script:Root $relative }
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Falta la entrada requerida: $path" }
    }
    $script:Version = Get-SourceVersion (Join-Path $script:Root 'src\main.rs')
    $toml = Get-Content -LiteralPath (Join-Path $script:Root 'Cargo.toml') -Raw
    $cargoVersion = [regex]::Match($toml, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"').Groups[1].Value
    Assert-True ($cargoVersion -eq $script:Version) 'Cargo.toml y src/main.rs difieren en versión.'
    $fixture = Get-Content -LiteralPath (Join-Path $script:Root 'tests\golden\cli_version.json') -Raw | ConvertFrom-Json -ErrorAction Stop
    Assert-True ($fixture.version -eq $script:Version -and $fixture.name -eq 'ai-voice-interconnector' -and $fixture.schema_version -eq '5') 'La versión fuente no coincide con fixture CLI schema 5.'
    if(!(Get-Command git -ErrorAction SilentlyContinue)){throw 'BLOCKED: git no está disponible para comprobar tag/version.'}
    if(!(Get-Command cargo -ErrorAction SilentlyContinue)){throw 'BLOCKED: cargo no está disponible para ejecutar el gate Rust actual.'}
    $releaseTag=(& git -C $script:Root describe --tags --abbrev=0 2>$null).Trim()
    if($LASTEXITCODE -ne 0 -or $releaseTag -ne "v$script:Version"){throw 'BLOCKED: tag actual, VERSION fuente y fixture deben coincidir antes de cualquier acción.'}
    $script:ProgramDir = Join-Path $env:LOCALAPPDATA 'Programs\ai-voice-interconnector'
    $script:Bin = Join-Path $script:ProgramDir 'ai-voice-interconnector.exe'

    $base = Join-Path ([IO.Path]::GetTempPath()) 'ai-voice-interconnector-e2e-evidence'
    [void](New-Item -ItemType Directory -Path $base -Force)
    $runPath = Join-Path $base ([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [guid]::NewGuid().ToString('N'))
    [void](New-Item -ItemType Directory -Path $runPath)
    $script:Run = $runPath
    $runFull=[IO.Path]::GetFullPath($script:Run)
    foreach($forbidden in @($script:ProgramDir,(Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector'),(Join-Path $env:LOCALAPPDATA 'Programs'))){$full=[IO.Path]::GetFullPath($forbidden).TrimEnd('\');if($runFull.Equals($full,[StringComparison]::OrdinalIgnoreCase)-or $runFull.StartsWith($full+'\',[StringComparison]::OrdinalIgnoreCase)){throw "La carpeta de evidencia se solapa con raíces de usuario/producto: $full"}}
    $preflightWatch.Stop()
    Set-Step 'preflight' 'PASS' 'Host, prerequisitos, fuentes y sintaxis AST validados.' -Command 'Initialize-Run + Parser::ParseFile' -Payload @{host=$env:OS;architecture='x64';powershell=$PSVersionTable.PSVersion.ToString();source_version=$script:Version;modules_repaired=$modulesRepaired} -Evidence @($PSCommandPath) -DurationMs $preflightWatch.ElapsedMilliseconds
    Write-Result 'INFO' "Evidencia: $runPath"
    Write-Result 'INFO' "Versión fuente/fixture: $script:Version"
}

function Save-Event {
    param([Parameter(Mandatory)][hashtable]$Event)
    Add-Content -LiteralPath (Join-Path $script:Run 'events.jsonl') -Value ($Event | ConvertTo-Json -Depth 30 -Compress) -Encoding UTF8
}

function Set-Step {
    param(
        [Parameter(Mandatory)][string]$Step,
        [Parameter(Mandatory)][ValidateSet('PENDING','RUNNING','PASS','FAIL','SKIP','BLOCKED','NOT_RUN')][string]$Status,
        [string]$Detail = '',
        [string]$Command = '',
        [Nullable[int]]$ExitCode = $null,
        $Payload = $null,
        [string[]]$Evidence = @(),
        [string]$Reason = '',
        [Nullable[long]]$DurationMs = $null
    )
    $formattedDuration = if ($null -ne $DurationMs) { Format-Duration $DurationMs } else { $null }
    $existing=@($script:Results|Where-Object step -eq $Step)
    if($Status -eq 'RUNNING'){
        if($existing.Count -eq 0){$script:Results.Add([pscustomobject]@{step=$Step;status='PENDING';detail='';command='';exit_code=$null;payload=$null;evidence=@();reason='';duration_ms=$null;duration_formatted=$null;utc=[DateTime]::UtcNow.ToString('o')})}
        $entry=@($script:Results|Where-Object step -eq $Step|Select-Object -Last 1)[0];$entry.status='RUNNING';$entry.detail=$Detail;$entry.duration_ms=$null;$entry.duration_formatted=$null;$entry.utc=[DateTime]::UtcNow.ToString('o')
    }elseif($existing.Count){$entry=$existing[-1];$entry.status=$Status;$entry.detail=$Detail;if($null -ne $DurationMs){$entry.duration_ms=$DurationMs;$entry.duration_formatted=$formattedDuration};$entry.utc=[DateTime]::UtcNow.ToString('o')}
    else{$script:Results.Add([pscustomobject]@{step=$Step;status=$Status;detail=$Detail;command='';exit_code=$null;payload=$null;evidence=@();reason='';duration_ms=$DurationMs;duration_formatted=$formattedDuration;utc=[DateTime]::UtcNow.ToString('o')})}
    $entry=@($script:Results|Where-Object step -eq $Step|Select-Object -Last 1)[0];$entry.command=$Command;$entry.exit_code=$ExitCode;$entry.payload=$Payload;$entry.evidence=@($Evidence);$entry.reason=$Reason
    $record = [pscustomobject]@{ step=$Step; status=$Status; detail=$Detail; command=$Command; exit_code=$ExitCode; payload=$Payload; evidence=@($Evidence); reason=$Reason; duration_ms=$DurationMs; duration_formatted=$formattedDuration; utc=[DateTime]::UtcNow.ToString('o') }
    Save-Event @{ type='step'; step=$Step; status=$Status; detail=$Detail; command=$Command; exit_code=$ExitCode; evidence=@($Evidence); reason=$Reason; duration_ms=$DurationMs; duration_formatted=$formattedDuration; utc=$record.utc }
    $existingSubcase=@($script:SubcaseResults|Where-Object name -eq $Step);if($existingSubcase.Count){$null=$script:SubcaseResults.Remove($existingSubcase[-1])};$script:SubcaseResults.Add([pscustomobject]@{name=$Step;status=$Status;command=$Command;exit_code=$ExitCode;payload=$Payload;evidence=@($Evidence);reason=$Reason;duration_ms=$DurationMs;duration_formatted=$formattedDuration;utc=$record.utc})
    $durationSuffix = if ($formattedDuration) { " [$formattedDuration]" } else { "" }
    Write-Result $Status "$Step $Detail$durationSuffix"
}

function ConvertTo-NativeArgument {
    param([AllowEmptyString()][string]$Value)
    if ($Value -notmatch '[\s"]') { return $Value }
    $escaped = $Value -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

function Stop-OwnedProcessTree {
    param([Parameter(Mandatory)][int]$ProcessId)
    $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$ProcessId" -ErrorAction SilentlyContinue
    foreach ($child in $children) { Stop-OwnedProcessTree -ProcessId ([int]$child.ProcessId) }
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($process) { Stop-Process -Id $ProcessId -Force -ErrorAction Stop }
}

function Protect-LogText {
    param([string]$Text)
    $protected=$Text
    foreach($name in @('GH_TOKEN','GITHUB_TOKEN','AVI_TOKEN','HF_TOKEN','HUGGINGFACE_TOKEN','AVI_PASSWORD')){$value=[Environment]::GetEnvironmentVariable($name);if($value){$protected=$protected.Replace($value,'[REDACTED]')}}
    return $protected
}

function Invoke-LoggedProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter(Mandatory)][string[]]$ArgumentList,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][int]$TimeoutMs,
        [string]$WorkingDirectory = $script:Root,
        [hashtable]$Environment = @{},
        [switch]$PersistedPath,
        [switch]$KillOnTimeout,
        [bool]$SanitizeOutput=$true
    )
    $safeName = $Name -replace '[^A-Za-z0-9_.-]', '_'
    $stdoutPath = Join-Path $script:Run "$safeName.stdout.log"
    $stderrPath = Join-Path $script:Run "$safeName.stderr.log"
    [IO.File]::WriteAllText($stdoutPath, '')
    [IO.File]::WriteAllText($stderrPath, '')

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    if ($PSVersionTable.PSVersion.Major -ge 7 -and $psi.PSObject.Properties['ArgumentList']) { $psi.ArgumentList.Clear(); foreach($argument in $ArgumentList){$psi.ArgumentList.Add([string]$argument)} } else { $psi.Arguments = (($ArgumentList | ForEach-Object { ConvertTo-NativeArgument ([string]$_) }) -join ' ') }
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    if ($PersistedPath) {
        $machine = [Environment]::GetEnvironmentVariable('Path','Machine')
        $user = [Environment]::GetEnvironmentVariable('Path','User')
        $persisted = @($machine,$user | Where-Object { $_ }) -join ';'
        $psi.EnvironmentVariables['PATH'] = $persisted
        # Como el PATH, el PSModulePath persistido modela una sesión real de usuario y evita que el hijo PowerShell herede módulos de un padre PowerShell 7.
        $persistedModulePath = @([Environment]::GetEnvironmentVariable('PSModulePath','Machine'),[Environment]::GetEnvironmentVariable('PSModulePath','User') | Where-Object { $_ }) -join ';'
        if ($persistedModulePath) { $psi.EnvironmentVariables['PSModulePath'] = $persistedModulePath }
    }
    foreach ($key in $Environment.Keys) { $psi.EnvironmentVariables[$key] = [string]$Environment[$key] }

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    $outStream = [IO.File]::Open($stdoutPath, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    $errStream = [IO.File]::Open($stderrPath, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try {
        $null = $process.Start()
        $outTask = $process.StandardOutput.BaseStream.CopyToAsync($outStream)
        $errTask = $process.StandardError.BaseStream.CopyToAsync($errStream)
        Save-Event @{ type='process'; name=$Name; file=$FilePath; args=$ArgumentList; started_utc=[DateTime]::UtcNow.ToString('o'); pid=$process.Id; timeout_ms=$TimeoutMs; stdout=$stdoutPath; stderr=$stderrPath }
        $exited = $process.WaitForExit($TimeoutMs)
        if (-not $exited) {
            if ($KillOnTimeout) { Stop-OwnedProcessTree -ProcessId $process.Id }
            else { throw "Timeout ${TimeoutMs}ms en $Name (PID $($process.Id)); no se terminó porque no es un proceso de propiedad exclusiva." }
            $process.WaitForExit()
        }
        $null = $outTask.GetAwaiter().GetResult()
        $null = $errTask.GetAwaiter().GetResult()
        $outStream.Dispose(); $errStream.Dispose()
        $stdout = [IO.File]::ReadAllText($stdoutPath)
        $stderr = [IO.File]::ReadAllText($stderrPath)
        $sanitizedStdout=if($SanitizeOutput){Protect-LogText $stdout}else{$stdout}
        $sanitizedStderr=if($SanitizeOutput){Protect-LogText $stderr}else{$stderr}
        if($sanitizedStdout -ne $stdout){[IO.File]::WriteAllText($stdoutPath,$sanitizedStdout,[Text.UTF8Encoding]::new($false))}
        if($sanitizedStderr -ne $stderr){[IO.File]::WriteAllText($stderrPath,$sanitizedStderr,[Text.UTF8Encoding]::new($false))}
        $stdout=$sanitizedStdout;$stderr=$sanitizedStderr
        $code = if ($exited) { $process.ExitCode } else { $null }
        Save-Event @{ type='process_end'; name=$Name; ended_utc=[DateTime]::UtcNow.ToString('o'); exit_code=$code; timed_out=(-not $exited) }
        if (-not $exited) { throw "Timeout ${TimeoutMs}ms en $Name (PID $($process.Id)); evidencia preservada." }
        return [pscustomobject]@{ ExitCode=$code; Stdout=$stdout; Stderr=$stderr; Pid=$process.Id; StdoutPath=$stdoutPath; StderrPath=$stderrPath }
    } finally {
        $outStream.Dispose(); $errStream.Dispose(); $process.Dispose()
    }
}

function ConvertFrom-JsonStrict {
    param([Parameter(Mandatory)][string]$Text)
    $trimmed = $Text.Trim()
    if (!$trimmed) { throw 'stdout JSON vacío.' }
    try { return ConvertFrom-Json -InputObject $trimmed -ErrorAction Stop } catch { throw "JSON inválido: $($_.Exception.Message)" }
}

function Invoke-Cli {
    param([Parameter(Mandatory)][string[]]$Arguments, [Parameter(Mandatory)][string]$Name, [int]$TimeoutMs=120000, [int[]]$ExpectedExit=@(0), [switch]$Json=$true, [switch]$PersistedPath, [hashtable]$Environment=@{}, [switch]$KillOnTimeout, [bool]$SanitizeOutput=$true)
    if (!(Test-Path -LiteralPath $script:Bin -PathType Leaf)) { throw "CLI instalada ausente: $script:Bin" }
    $args = @($Arguments)
    if ($Json -and $args -notcontains '--json') { $args += '--json' }
    $caseName='cmd-'+$Name
    Set-Step $caseName 'RUNNING' ('{0} {1}' -f $script:Bin,($args -join ' '))
    try {
        $result = Invoke-LoggedProcess -FilePath $script:Bin -ArgumentList $args -Name $Name -TimeoutMs $TimeoutMs -Environment $Environment -PersistedPath:$PersistedPath -KillOnTimeout:$KillOnTimeout -SanitizeOutput:$SanitizeOutput
        $result | Add-Member -NotePropertyName Arguments -NotePropertyValue $args
        if ($result.ExitCode -notin $ExpectedExit) { throw "$Name devolvió exit $($result.ExitCode); esperado: $($ExpectedExit -join ',')." }
        if ($Json) { $result | Add-Member -NotePropertyName Payload -NotePropertyValue (ConvertFrom-JsonStrict $result.Stdout) }
        Set-Step $caseName 'PASS' 'Comando y payload parseados.' -Command ($script:Bin+' '+($args -join ' ')) -ExitCode $result.ExitCode -Payload $(if($Json){$result.Payload}else{$null}) -Evidence @($result.StdoutPath,$result.StderrPath)
        return $result
    } catch {
        $caseStatus=if($_.Exception.Message.StartsWith('BLOCKED:')){'BLOCKED'}else{'FAIL'}
        Set-Step $caseName $caseStatus $_.Exception.Message -Command ($script:Bin+' '+($args -join ' ')) -Evidence @((Join-Path $script:Run ($Name+'.stdout.log')),(Join-Path $script:Run ($Name+'.stderr.log'))) -Reason $_.Exception.Message
        throw
    }
}

function Get-PathEntries {
    $path = [Environment]::GetEnvironmentVariable('Path','User')
    if (!$path) { return @() }
    return @($path -split ';' | Where-Object { $_.Trim() })
}

function Assert-PayloadSchema {
    param($Payload)
    Assert-True ($Payload.schema_version -eq '5') 'schema_version debe ser 5.'
}

function Test-NativeSttCapability {
    param([string]$AudioPath)
    if(!(Test-Path -LiteralPath $AudioPath -PathType Leaf)){throw ('No existe fixture de audio para sondear native-stt: {0}' -f $AudioPath)}
    $probe=Invoke-Cli @('speech','transcribe','--audio',$AudioPath,'--source-language','es','--no-daemon') 'probe-native-stt-capability' -Json -ExpectedExit @(0,1,4)
    if($probe.ExitCode -eq 0){return $true}
    if($probe.Payload.reason -eq 'stt_unsupported'){return $false}
    if($probe.Payload.reason -eq 'model_missing'){return $false}
    throw 'La sonda de native-stt falló por una causa distinta de capacidad/modelo ausentes.'
}

function Get-ModelSnapshot {
    param([string]$ModelName,[string]$Repo,[string]$Revision,[string[]]$RequiredFiles)
    $repoPath='models--'+($Repo -replace '/','--')
    $snapshot=Join-Path (Join-Path (Join-Path $script:ModelsDir $repoPath) ('snapshots\'+$Revision)) ''
    if(!(Test-Path -LiteralPath $snapshot -PathType Container)){throw ('Snapshot pinneado ausente para {0}: {1}' -f $ModelName,$snapshot)}
    foreach($relative in $RequiredFiles){$file=Join-Path $snapshot $relative;if(!(Test-Path -LiteralPath $file -PathType Leaf)-or (Get-Item -LiteralPath $file).Length -le 0){throw ('Archivo requerido ausente/vacío para {0}: {1}' -f $ModelName,$file)}}
    return $snapshot
}

function Set-SubcaseResult {
    param([string]$Name,[ValidateSet('PENDING','RUNNING','PASS','FAIL','SKIP','BLOCKED')][string]$Status,[string]$Command='',[Nullable[int]]$ExitCode=$null,$Payload=$null,[string[]]$Evidence=@(),[string]$Reason='')
    $record=[pscustomobject]@{name=$Name;status=$Status;command=$Command;exit_code=$ExitCode;payload=$Payload;evidence=@($Evidence);reason=$Reason;utc=[DateTime]::UtcNow.ToString('o')}
    $prior=@($script:SubcaseResults|Where-Object name -eq $Name)
    if($prior.Count){$null=$script:SubcaseResults.Remove($prior[-1])}
    $script:SubcaseResults.Add($record)
    Save-Event @{type='subcase';name=$Name;status=$Status;command=$Command;exit_code=$ExitCode;evidence=@($Evidence);reason=$Reason;utc=$record.utc}
}


function Invoke-WhereCheck {
    param([Parameter(Mandatory)][bool]$ExpectPresent, [switch]$AllowPreSetup)
    $powershell = Join-Path $PSHOME $(if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh.exe' } else { 'powershell.exe' })
    $scriptText = if ($ExpectPresent) {
        if ($AllowPreSetup) {
            '$p = (where.exe ai-voice-interconnector 2>$null); if ($LASTEXITCODE -ne 0 -or -not ($p -contains ''{0}'')) {{ exit 10 }}; $version = (& ''{0}'' version --json | ConvertFrom-Json -ErrorAction Stop); if ($LASTEXITCODE -ne 0 -or $version.version -ne ''{1}'' -or $version.schema_version -ne ''5'') {{ exit 12 }}; exit 0' -f $script:Bin.Replace("'", "''"),$script:Version
        } else {
            '$p = (where.exe ai-voice-interconnector 2>$null); if ($LASTEXITCODE -ne 0 -or -not ($p -contains ''{0}'')) {{ exit 10 }}; $version = (& ''{0}'' version --json | ConvertFrom-Json -ErrorAction Stop); if ($LASTEXITCODE -ne 0 -or $version.version -ne ''{1}'' -or $version.schema_version -ne ''5'') {{ exit 12 }}; & ''{0}'' doctor --json; exit $LASTEXITCODE' -f $script:Bin.Replace("'", "''"),$script:Version
        }
    } else {
        'where.exe ai-voice-interconnector *> $null; if ($LASTEXITCODE -eq 1) { exit 0 } else { exit 11 }'
    }
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($scriptText))
    $result = Invoke-LoggedProcess -FilePath $powershell -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand',$encoded) -Name ('where-' + [guid]::NewGuid().ToString('N')) -TimeoutMs 30000 -PersistedPath
    if ($ExpectPresent -and -not $AllowPreSetup -and $result.ExitCode -eq 0) {
        $doctor = ConvertFrom-JsonStrict $result.Stdout
        Assert-True (@($doctor.failed).Count -eq 0 -and $doctor.path.resolves_to_this_install -eq $true -and @($doctor.path.duplicate_entries).Count -eq 0) 'Doctor del hijo con PATH persistido reporta fallos, duplicados o ruta incorrecta.'
        foreach($key in @('unknown_owner','pending_failures','unreadable')){
            $pendingValues=@($doctor.pending.PSObject.Properties[$key].Value)
            Assert-True ($pendingValues.Count -eq 0) ('Doctor hijo reporta pending {0}.' -f $key)
        }
        Assert-True (@($doctor.models.missing).Count -eq 0) 'Doctor hijo reporta modelos faltantes.'
        $result | Add-Member -NotePropertyName Doctor -NotePropertyValue $doctor
    }
    return $result
}

function Get-ReceiptPath {
    return Join-Path $script:ProgramDir 'install-receipt.json'
}

function Get-RegisteredRoots {
    param([switch]$AllowMissing)
    $receiptPath = Get-ReceiptPath
    if (!(Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
        if ($AllowMissing) { return $null }
        throw ('Falta receipt: {0}' -f $receiptPath)
    }
    $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -ErrorAction Stop
    Assert-True ($receipt.schema_version -eq 2 -and $receipt.app -eq 'ai-voice-interconnector') 'Receipt ilegible o de esquema/app no reconocido.'
    Assert-True ($receipt.install_dir -eq $script:ProgramDir) 'Receipt no es propietario del directorio esperado.'
    foreach($key in @('data_dir','cache_dir')){if([string]::IsNullOrWhiteSpace([string]$receipt.roots.$key)){throw ('Receipt no define roots.{0}.' -f $key)}}
    return $receipt
}

function Get-ReleaseInfo {
    $uri = 'https://api.github.com/repos/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tags/v{0}' -f $script:Version
    $release = Invoke-RestMethod -Uri $uri -Headers @{ 'User-Agent'='avi-windows-e2e' } -TimeoutSec 30
    $expected = @(('ai-voice-interconnector-{0}-x86_64-windows.zip' -f $script:Version), ('ai-voice-interconnector-{0}-x86_64-linux.tar.gz' -f $script:Version), ('ai-voice-interconnector-{0}-arm64-linux.tar.gz' -f $script:Version), ('ai-voice-interconnector-{0}-arm64-macos.tar.gz' -f $script:Version), 'install.sh', 'install.ps1', 'SHA256SUMS.txt')
    $names = @($release.assets | ForEach-Object name)
    foreach ($name in $expected) { if($names -notcontains $name){throw ('Falta asset publicado: {0}' -f $name)} }
    if($names.Count -ne 7){throw ('Se esperaban siete assets; hay {0}.' -f $names.Count)}
    Assert-True ($release.draft -eq $false -and $release.prerelease -eq $false) 'El release consultado no es estable/publicado.'
    $installAsset = $release.assets | Where-Object name -eq 'install.ps1' | Select-Object -First 1
    Assert-True ($installAsset.browser_download_url -match ('/releases/download/v'+[regex]::Escape($script:Version)+'/install\.ps1$')) 'La URL del bootstrap no pertenece al asset del tag verificado.'
    return $release
}

function Get-DryRunTargets {
    $cleanup = Invoke-Cli @('cleanup','--all','--dry-run') 'cleanup-dry-run' -Json
    $uninstall = Invoke-Cli @('self','uninstall','--dry-run') 'uninstall-dry-run' -Json
    foreach($pair in @(@{payload=$cleanup.Payload;kind='cleanup'},@{payload=$uninstall.Payload;kind='uninstall'})){
        Assert-PayloadSchema $pair.payload
        foreach($name in @('planned','preserved')){
            $property=$pair.payload.PSObject.Properties[$name]
            if(!$property -or $property.Value -isnot [array]){throw ('BLOCKED: {0} --dry-run no expone {1} como array.' -f $pair.kind,$name)}
        }
        foreach($entry in $pair.payload.preserved){
            if($entry -isnot [pscustomobject] -or !$entry.PSObject.Properties['path'] -or !$entry.PSObject.Properties['reason'] -or $entry.path -isnot [string] -or $entry.reason -isnot [string] -or [string]::IsNullOrWhiteSpace($entry.path) -or ![IO.Path]::IsPathRooted($entry.path) -or [string]::IsNullOrWhiteSpace($entry.reason)){throw ('BLOCKED: {0} --dry-run contiene un preservado inválido.' -f $pair.kind)}
        }
        foreach($path in @($pair.payload.planned)){
            if($path -isnot [string] -or [string]::IsNullOrWhiteSpace($path) -or ![IO.Path]::IsPathRooted($path)){throw 'BLOCKED: dry-run contiene una ruta inválida.'}
            $full=[IO.Path]::GetFullPath([string]$path)
            $allowed=@($script:ProgramDir,$script:DataDir,$script:CacheDir)|Where-Object{$_}
            $owned=$false
            foreach($root in $allowed){$rootFull=[IO.Path]::GetFullPath([string]$root).TrimEnd('\');if($full.Equals($rootFull,[StringComparison]::OrdinalIgnoreCase)-or $full.StartsWith($rootFull+'\',[StringComparison]::OrdinalIgnoreCase)){$owned=$true;break}}
            $container=[IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector')).TrimEnd('\')
            if(!$owned -and $full.Equals($container,[StringComparison]::OrdinalIgnoreCase) -and @($script:DataDir,$script:CacheDir|Where-Object{[IO.Path]::GetFullPath($_).StartsWith($container+'\',[StringComparison]::OrdinalIgnoreCase)}).Count -eq 2){$owned=$true}
            if(!$owned){throw ('BLOCKED: dry-run anunció destino fuera de raíces de propiedad: {0}' -f $full)}
        }
        foreach($entry in $pair.payload.preserved){
            $preserved=[IO.Path]::GetFullPath($entry.path).TrimEnd('\')
            foreach($path in $pair.payload.planned){$target=[IO.Path]::GetFullPath($path).TrimEnd('\');if($preserved.Equals($target,[StringComparison]::OrdinalIgnoreCase) -or $preserved.StartsWith($target+'\',[StringComparison]::OrdinalIgnoreCase)){throw ('BLOCKED: {0} anuncia preservado dentro de un destino: {1}' -f $pair.kind,$preserved)}}
        }
    }
    return [pscustomobject]@{ Cleanup=$cleanup.Payload; Uninstall=$uninstall.Payload; CleanupArguments=$cleanup.Arguments; UninstallArguments=$uninstall.Arguments; CleanupExitCode=$cleanup.ExitCode; UninstallExitCode=$uninstall.ExitCode; CleanupStdoutPath=$cleanup.StdoutPath; UninstallStdoutPath=$uninstall.StdoutPath; CleanupStderrPath=$cleanup.StderrPath; UninstallStderrPath=$uninstall.StderrPath }
}

function Normalize-TargetSet {
    param([string[]]$Paths)
    $list = [System.Collections.Generic.List[string]]::new()
    foreach ($p in @($Paths)) {
        if ($p -and -not [string]::IsNullOrWhiteSpace([string]$p)) {
            $normalized = [IO.Path]::GetFullPath([string]$p).TrimEnd('\')
            if (-not $list.Contains($normalized)) { [void]$list.Add($normalized) }
        }
    }
    $list.Sort()
    return $list.ToArray()
}

function Assert-TargetSetsEqual {
    param([string]$Operation,[string[]]$Expected,[string[]]$Current)
    $expectedSet=Normalize-TargetSet $Expected
    $currentSet=Normalize-TargetSet $Current
    $difference=@(Compare-Object -ReferenceObject $expectedSet -DifferenceObject $currentSet -CaseSensitive:$false)
    if($difference.Count){throw ('BLOCKED: el plan efectivo de {0} difiere de su dry-run. Dry-run=[{1}] Efectivo=[{2}]' -f $Operation,($expectedSet -join '; '),($currentSet -join '; '))}
}

function Get-DirectorySize {
    param([string]$Path)
    if (!(Test-Path -LiteralPath $Path)) { return [long]0 }
    $files = @(Get-ChildItem -LiteralPath $Path -File -Recurse -Force -ErrorAction Stop)
    if (!$files.Count) { return [long]0 }
    $sum = ($files | Measure-Object -Property Length -Sum).Sum
    if($null -eq $sum){return [long]0}
    return [long]$sum
}

function Invoke-Step {
    param([Parameter(Mandatory)][string]$Id, [Parameter(Mandatory)][scriptblock]$Action)
    Assert-True (@($script:Phases.Id) -contains $Id) ('Fase desconocida fuera de $script:Phases: {0}' -f $Id)
    Set-Step $Id 'RUNNING'
    $stepWatch = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        & $Action
        $stepWatch.Stop()
        Set-Step $Id 'PASS' -DurationMs $stepWatch.ElapsedMilliseconds
    }
    catch {
        $stepWatch.Stop()
        $status = if ($_.Exception.Message.StartsWith('BLOCKED:')) { 'BLOCKED' } else { 'FAIL' }
        Set-Step $Id $status $_.Exception.Message -DurationMs $stepWatch.ElapsedMilliseconds
        throw
    }
}

function Start-ManagedDaemon {
    $before = Invoke-Cli @('daemon','status') 'daemon-status-before' -Json
    Assert-PayloadSchema $before.Payload
    Assert-True ($before.Payload.daemon -eq 'stopped') 'El daemon debía estar stopped antes del start.'
    $start = Invoke-Cli @('daemon','start') 'daemon-start' -TimeoutMs 15000 -Json
    Assert-PayloadSchema $start.Payload
    Assert-True ($start.Payload.daemon -eq 'running' -and $start.Payload.status -eq 'started') 'Start debe iniciar un daemon detenido y reportar started; already_running no es una carrera válida en este flujo.'
    $script:DaemonOwned = $true
    $script:DaemonPid = [int]$start.Payload.pid
    $process=Get-Process -Id $script:DaemonPid -ErrorAction Stop
    Assert-True ($process.ProcessName -eq 'ai-voice-interconnector') 'PID reportado no pertenece al ejecutable daemon esperado.'
    $processPath=(Get-CimInstance Win32_Process -Filter ('ProcessId={0}' -f $script:DaemonPid) -ErrorAction Stop).ExecutablePath
    Assert-True ([IO.Path]::GetFullPath($processPath) -ieq [IO.Path]::GetFullPath($script:Bin)) 'PID daemon resuelve a un ejecutable ajeno al bundle instalado.'
    $script:DaemonOwned=$true
    [void]$script:DaemonStartedIdentities.Add($script:DaemonPid)
    $deadline = [DateTime]::UtcNow.AddMinutes(3)
    do {
        $current=Get-Process -Id $script:DaemonPid -ErrorAction SilentlyContinue
        Assert-True ($null -ne $current) 'PID daemon iniciado por esta corrida dejó de existir durante la espera warm.'
        $currentPath=(Get-CimInstance Win32_Process -Filter ('ProcessId={0}' -f $script:DaemonPid) -ErrorAction Stop).ExecutablePath
        Assert-True ([IO.Path]::GetFullPath($currentPath) -ieq [IO.Path]::GetFullPath($script:Bin)) 'PID daemon cambió de identidad durante la espera warm.'
        $status = Invoke-Cli @('daemon','status') 'daemon-status-poll' -TimeoutMs 10000 -Json
        if ($status.Payload.daemon -eq 'running' -and $status.Payload.warm -eq 'warm') { break }
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Daemon no alcanzó warm dentro de 3 minutos.' }
        Start-Sleep -Milliseconds 500
    } while ($true)
    $port = if ($env:AVI_DAEMON_PORT) { [int]$env:AVI_DAEMON_PORT } else { 8765 }
    $healthUri = 'http://127.0.0.1:{0}/health' -f $port
    try { $health = Invoke-RestMethod -Uri $healthUri -TimeoutSec 5 } catch { throw ('Health del daemon no disponible: {0}' -f $_.Exception.Message) }
    Assert-True ($health.status -eq 'ready' -and $health.warm -eq 'warm') 'Health del daemon no confirma estado listo.'
}

function Stop-ManagedDaemon {
    Assert-True ($script:DaemonOwned -and $script:DaemonPid -and $script:DaemonStartedIdentities.Contains([int]$script:DaemonPid)) 'No existe identidad de daemon creada y registrada por esta corrida para detener.'
    $process=Get-Process -Id $script:DaemonPid -ErrorAction Stop
    $processPath=(Get-CimInstance Win32_Process -Filter ('ProcessId={0}' -f $script:DaemonPid) -ErrorAction Stop).ExecutablePath
    Assert-True ([IO.Path]::GetFullPath($processPath) -ieq [IO.Path]::GetFullPath($script:Bin)) 'La identidad del daemon cambió; no se enviará stop a un proceso ajeno.'
    $stop = Invoke-Cli @('daemon','stop') 'daemon-stop' -TimeoutMs 30000 -Json
    Assert-PayloadSchema $stop.Payload
    Assert-True ($stop.Payload.daemon_fully_stopped -eq $true -and $stop.Payload.daemon -eq 'stopped') 'Stop del daemon no quedó completamente verificado.'
    $status = Invoke-Cli @('daemon','status') 'daemon-status-after-stop' -Json
    Assert-True ($status.Payload.daemon -eq 'stopped') 'El daemon continúa activo después de stop.'
    $script:DaemonOwned = $false
    $script:DaemonPid = $null
}

function Invoke-VoiceClone {
    param([string]$Name, [switch]$Daemon, [switch]$Denoise, [switch]$Mic, [switch]$Force, [int[]]$ExpectedExit=@(0))
    $args = @('voice','clone','--name',$Name)
    if ($Mic) { $args += @('--mic','--duration','5') } else { $args += @('--speech-reference',$script:Reference) }
    if ($Daemon) { $args += '--daemon' } else { $args += '--no-daemon' }
    if ($Denoise) { $args += '--denoise' }
    if ($Force) { $args += '--force' }
    $args += '--json'
    $commandName='clone-{0}' -f $Name
    $result = Invoke-Cli -Arguments $args -Name $commandName -Json -ExpectedExit $ExpectedExit
    if ($result.ExitCode -eq 0 -and !$Mic) {
        $qvoicePath=[string]$result.Payload.speech
        if(!(Test-Path -LiteralPath $qvoicePath -PathType Leaf)){throw ('El payload speech no señala el qvoice escrito: {0}' -f $qvoicePath)}
        $result | Add-Member -NotePropertyName QvoicePath -NotePropertyValue $qvoicePath
    }
    return $result
}

function Assert-Wav {
    param([Parameter(Mandatory)][string]$Path)
    if(!(Test-Path -LiteralPath $Path -PathType Leaf)){throw ('No existe WAV: {0}' -f $Path)}
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = New-Object IO.BinaryReader($stream)
        $riff = [Text.Encoding]::ASCII.GetString($reader.ReadBytes(4)); $null = $reader.ReadUInt32()
        $wave = [Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
        Assert-True ($riff -eq 'RIFF' -and $wave -eq 'WAVE') ('No es WAV RIFF: {0}' -f $Path)
        $format=0; $channels=0; $rate=0; $bits=0; $dataBytes=0
        while ($stream.Position + 8 -le $stream.Length) {
            $chunk = [Text.Encoding]::ASCII.GetString($reader.ReadBytes(4)); $length = [long]$reader.ReadUInt32()
            if ($stream.Position + $length -gt $stream.Length) { throw ('Chunk WAV truncado: {0}' -f $Path) }
            if ($chunk -eq 'fmt ') {
                if ($length -lt 16) { throw ('Chunk fmt WAV inválido: {0}' -f $Path) }
                $format=$reader.ReadUInt16(); $channels=$reader.ReadUInt16(); $rate=$reader.ReadUInt32(); $null=$reader.ReadUInt32(); $null=$reader.ReadUInt16(); $bits=$reader.ReadUInt16(); $stream.Position += ($length-16)
            } elseif ($chunk -eq 'data') { $dataBytes=$length; $stream.Position += $length } else { $stream.Position += $length }
            if (($length % 2) -ne 0) { $stream.Position++ }
        }
        Assert-True ($format -eq 1 -and $channels -eq 1 -and $rate -eq 24000 -and $bits -eq 16 -and $dataBytes -gt 0) ('WAV debe ser PCM16 mono 24kHz con muestras: {0}' -f $Path)
    } finally { $stream.Dispose() }
}

function Get-Wer {
    param([string]$Reference, [string]$Hypothesis)
    function Normalize-Words([string]$Text) {
        $builder = New-Object Text.StringBuilder
        foreach ($ch in $Text.ToLowerInvariant().ToCharArray()) {
            $mapped = switch ([int]$ch) {
                225 { 'a' } 228 { 'a' } 233 { 'e' } 235 { 'e' } 237 { 'i' } 239 { 'i' } 243 { 'o' } 246 { 'o' } 250 { 'u' } 252 { 'u' } 241 { 'n' }
                default { if ([int]$ch -le 127 -and [char]::IsLetterOrDigit($ch)) { [string]$ch } else { ' ' } }
            }
            [void]$builder.Append($mapped)
        }
        return @($builder.ToString() -split '\s+' | Where-Object { $_ })
    }
    $a=@(Normalize-Words $Hypothesis); $b=@(Normalize-Words $Reference)
    if ($b.Count -eq 0) { return 1.0 }
    $prev=[int[]]::new($b.Count+1)
    for($j=0;$j -le $b.Count;$j++){$prev[$j]=$j}
    for($i=0;$i -lt $a.Count;$i++){
        $cur=[int[]]::new($b.Count+1);$cur[0]=$i+1
        for($j=0;$j -lt $b.Count;$j++){
            $cost=if($a[$i] -ceq $b[$j]){0}else{1}
            $cur[$j+1]=[Math]::Min([Math]::Min($cur[$j]+1,$prev[$j+1]+1),$prev[$j]+$cost)
        }
        $prev=$cur
    }
    return [double]$prev[$b.Count]/$b.Count
}

function Get-VoiceQvoice {
    param([Parameter(Mandatory)][string]$VoiceName)
    $path = Join-Path $script:DataDir ('voices\{0}\reference.qvoice' -f $VoiceName)
    if(!(Test-Path -LiteralPath $path -PathType Leaf)){throw ('Falta qvoice de {0}.' -f $VoiceName)}
    return $path
}

function Run-E2E {
    $script:DataDir=Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector\data'
    $script:CacheDir=Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector\cache'
    $script:ModelsDir=Join-Path $script:CacheDir 'models'
    $script:Reference=(Resolve-Path -LiteralPath (Join-Path $script:Root 'assets\audios\Speech Sampler.wav')).Path
    $sttSample=(Resolve-Path -LiteralPath (Join-Path $script:Root 'crates\avi-stt\tests\assets\parakeet_sample_16k.wav')).Path

    Invoke-Step '01-release' {
        $tag=(& git -C $script:Root describe --tags --abbrev=0 2>$null).Trim();if($LASTEXITCODE -ne 0){throw 'No se pudo resolver el tag más reciente.'}
        Assert-True ($tag -eq ('v{0}' -f $script:Version)) ('Tag {0} difiere de {1}.' -f $tag,$script:Version)
        $script:Release=Get-ReleaseInfo
        $assets=@($script:Release.assets|ForEach-Object name)
        $script:Release|ConvertTo-Json -Depth 15|Set-Content -LiteralPath (Join-Path $script:Run 'release.json') -Encoding UTF8
        $checksums=$script:Release.assets|Where-Object name -eq 'SHA256SUMS.txt'|Select-Object -First 1
        $checksumText=Invoke-RestMethod -Uri $checksums.browser_download_url -TimeoutSec 30
        $checksumFile=Join-Path $script:Run 'SHA256SUMS.txt';Set-Content -LiteralPath $checksumFile -Value $checksumText -Encoding UTF8
        $winAsset='ai-voice-interconnector-{0}-x86_64-windows.zip' -f $script:Version
        $hashPattern='(?m)^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($winAsset) + '$'
        $hashMatch=[regex]::Match([string]$checksumText,$hashPattern)
        Assert-True ($hashMatch.Success) 'SHA256SUMS del release no contiene el asset Windows esperado.'
        $script:ExpectedWindowsSha256=$hashMatch.Groups[1].Value.ToLowerInvariant()
        Save-Event @{type='release';tag=$tag;assets=$assets;windows_asset=$winAsset;expected_sha256=$script:ExpectedWindowsSha256}
    }

    Invoke-Step '02-source-gate' {
        $rustGate=Invoke-LoggedProcess -FilePath (Get-Command cargo -ErrorAction Stop).Source -ArgumentList @('test','--test','cli_golden','version_matches_fixture','--','--exact','--nocapture') -Name 'source-version-fixture-gate' -TimeoutMs 600000
        $rustSummary=[regex]::Match(($rustGate.Stdout+$rustGate.Stderr),'(?m)^test result: (?<status>[^.]+)\. (?<passed>\d+) passed; (?<failed>\d+) failed; (?<ignored>\d+) ignored; (?<measured>\d+) measured; (?<filtered>\d+) filtered out;')
        Assert-True ($rustGate.ExitCode -eq 0 -and $rustSummary.Success -and $rustSummary.Groups['passed'].Value -eq '1' -and $rustSummary.Groups['failed'].Value -eq '0') 'Gate Rust version_matches_fixture no pasó o no entregó resumen parseable.'
        Save-Event @{type='source_test_gate';command='cargo test --test cli_golden version_matches_fixture -- --exact --nocapture';features=@();exit_code=$rustGate.ExitCode;passed=[int]$rustSummary.Groups['passed'].Value;failed=[int]$rustSummary.Groups['failed'].Value;ignored=[int]$rustSummary.Groups['ignored'].Value;stdout=$rustGate.StdoutPath;stderr=$rustGate.StderrPath}
    }

    Invoke-Step '03-cleanup' {
        $receiptPath=Get-ReceiptPath; $binaryExists=Test-Path -LiteralPath $script:Bin -PathType Leaf
        $registered=Get-RegisteredRoots -AllowMissing
        $programDir=Split-Path -Parent $script:Bin
        $dataDir=Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector\data'; $cacheRoot=Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector\cache'
        foreach($variable in @('AVI_DATA_DIR','AVI_CACHE_DIR','HF_HUB_CACHE','HF_HOME')){if([Environment]::GetEnvironmentVariable($variable)){throw ('BLOCKED: {0} modifica el alcance de las raíces del receipt; no se continuará con rutas ambiguas.' -f $variable)}}
        if($registered){$programDir=[string]$registered.install_dir;$dataDir=[string]$registered.roots.data_dir;$cacheRoot=[string]$registered.roots.cache_dir;$script:DataDir=$dataDir;$script:CacheDir=$cacheRoot}
        $ownedPaths=@($programDir,$dataDir,$cacheRoot,(Join-Path $cacheRoot 'tmp'),(Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector\scratch'),(Join-Path $env:LOCALAPPDATA 'Programs\.ai-voice-interconnector-staging-*'),(Join-Path $programDir '.old-*'))
        $existingPaths=@()
        foreach($candidate in $ownedPaths){if($candidate.Contains('*')){$existingPaths+=@(Get-Item -Path $candidate -Force -ErrorAction SilentlyContinue|Select-Object -ExpandProperty FullName)}elseif(Test-Path -LiteralPath $candidate){$existingPaths+=$candidate}}
        $ownedPaths=@($existingPaths|Select-Object -Unique)
        $residue=@($ownedPaths|Where-Object{Test-Path -LiteralPath $_});$pathEntries=@(Get-PathEntries)
        $matchingPath=@($pathEntries|Where-Object{$_.TrimEnd('\') -ieq $programDir.TrimEnd('\')})
        $inventory=[pscustomobject]@{binary=$binaryExists;receipt=(Test-Path -LiteralPath $receiptPath);roots_from_receipt=($null -ne $registered);paths=@($ownedPaths|ForEach-Object{[pscustomobject]@{path=$_;exists=(Test-Path -LiteralPath $_);bytes=(Get-DirectorySize $_)}});user_path=$pathEntries;matching_path_count=$matchingPath.Count}
        $inventory|ConvertTo-Json -Depth 12|Set-Content -LiteralPath (Join-Path $script:Run 'inventory.json') -Encoding UTF8
        if($binaryExists -and !$registered){throw 'BLOCKED: hay binario pero falta receipt; no se asume propiedad ni se ejecuta self uninstall.'}
        if(!$binaryExists -and ($residue.Count -or $matchingPath.Count)){throw 'BLOCKED: binario ausente con residuos o PATH integrado; no se presume ownership ni se borra manualmente.'}
        if($binaryExists){
            if($registered.roots.data_dir -eq $registered.roots.cache_dir){throw 'BLOCKED: raíces data/cache coinciden; no se puede determinar alcance seguro.'}
            if($registered.install_dir -ne $script:ProgramDir){throw 'BLOCKED: el receipt registra un directorio de programa no canónico; no se ejecutará cleanup E2E.'}
            $effectiveModelsRoot=if($env:HF_HUB_CACHE){$env:HF_HUB_CACHE}elseif($env:HF_HOME){Join-Path $env:HF_HOME 'hub'}else{Join-Path $registered.roots.cache_dir 'models'}
            if([IO.Path]::GetFullPath($effectiveModelsRoot).TrimEnd('\') -ne [IO.Path]::GetFullPath((Join-Path $registered.roots.cache_dir 'models')).TrimEnd('\')){throw 'BLOCKED: la raíz efectiva de modelos no coincide con la raíz propia registrada; no se purga caché compartida.'}
            $dry=Get-DryRunTargets;$dry|ConvertTo-Json -Depth 30|Set-Content -LiteralPath (Join-Path $script:Run 'dry-runs.json') -Encoding UTF8;Write-Host ($dry|ConvertTo-Json -Depth 20)
            $script:CleanupTargetSpecs=@(@($dry.Cleanup.planned)|Where-Object{$_ -and -not [string]::IsNullOrWhiteSpace($_)}|ForEach-Object{[pscustomobject]@{path=[string]$_;bytes=(Get-DirectorySize ([string]$_))}})
            $script:UninstallTargetSpecs=@(@($dry.Uninstall.planned)|Where-Object{$_ -and -not [string]::IsNullOrWhiteSpace($_)}|ForEach-Object{[pscustomobject]@{path=[string]$_;bytes=(Get-DirectorySize ([string]$_))}})
            $cleanupTargets=@(@($script:CleanupTargetSpecs|Where-Object{$_})|ForEach-Object path)
            $uninstallTargets=@(@($script:UninstallTargetSpecs|Where-Object{$_})|ForEach-Object path)
            $cleanupTargets=@(Normalize-TargetSet $cleanupTargets)
            $uninstallTargets=@(Normalize-TargetSet $uninstallTargets)
            $targets=@(Normalize-TargetSet @($cleanupTargets+$uninstallTargets))
            if($targets|Where-Object{$_ -match '(?i)huggingface|\.cache[\\/]huggingface'}){throw 'BLOCKED: el dry-run incluiría caché Hugging Face compartida.'}
            $ownershipRoots=@($registered.install_dir,$registered.roots.data_dir,$registered.roots.cache_dir)|ForEach-Object{[IO.Path]::GetFullPath([string]$_).TrimEnd('\')}
            if(!$registered.path_integration.modify_path -and $matchingPath.Count){throw 'BLOCKED: PATH contiene la ruta instalada pero el receipt no registra esa integración.'}
            $container=[IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'ai-voice-interconnector')).TrimEnd('\')
            foreach($target in $targets){$full=[IO.Path]::GetFullPath($target);$owned=$false;foreach($root in $ownershipRoots){if($full.Equals($root,[StringComparison]::OrdinalIgnoreCase)-or $full.StartsWith($root+'\',[StringComparison]::OrdinalIgnoreCase)){$owned=$true;break}};if(!$owned -and $full.Equals($container,[StringComparison]::OrdinalIgnoreCase) -and @($dataDir,$cacheRoot|Where-Object{[IO.Path]::GetFullPath($_).StartsWith($container+'\',[StringComparison]::OrdinalIgnoreCase)}).Count -eq 2){$owned=$true};if(!$owned){throw ('BLOCKED: dry-run anunció destino fuera de raíces registradas: {0}' -f $full)}}
            if(($residue.Count -or $matchingPath.Count) -and !$targets.Count){throw 'BLOCKED: los dry-runs no enumeraron destinos; sin ejecución destructiva.'}
            foreach($path in $residue){$fullResidue=[IO.Path]::GetFullPath($path).TrimEnd('\');$covered=@($targets|Where-Object{$fullTarget=[IO.Path]::GetFullPath($_).TrimEnd('\');$fullResidue.Equals($fullTarget,[StringComparison]::OrdinalIgnoreCase)-or $fullResidue.StartsWith($fullTarget+'\',[StringComparison]::OrdinalIgnoreCase)});if(!$covered.Count){throw ('BLOCKED: residuo inventariado no cubierto por un destino oficial: {0}' -f $path)}}
            if($targets|Where-Object{$_ -match '(?i)huggingface|\.cache[\\/]huggingface'}){throw 'BLOCKED: el dry-run incluiría caché Hugging Face compartida.'}
            if(@($targets).Count -or @($residue).Count -or @($matchingPath).Count){
                $targetSizeMap=@{}
                foreach($spec in @($script:CleanupTargetSpecs+$script:UninstallTargetSpecs|Where-Object{$_})){$targetSizeMap[$spec.path]=$spec.bytes}
                $targetSizes=@($targetSizeMap.GetEnumerator()|ForEach-Object{[pscustomobject]@{path=$_.Key;bytes=$_.Value}})
                foreach($target in @($targetSizes|Where-Object{$_})){$sizeMessage='DESTINO {0} - {1} bytes' -f $target.path,([long]$target.bytes);Write-Host $sizeMessage -ForegroundColor Yellow}
                foreach($pair in @(@{kind='cleanup';payload=$dry.Cleanup},@{kind='uninstall';payload=$dry.Uninstall})){foreach($preserved in @($pair.payload.preserved|Where-Object{$_})){$preservedSize=Get-DirectorySize $preserved.path;Write-Host ('PRESERVADO {0}: {1} - {2} bytes - {3}' -f $pair.kind,$preserved.path,$preservedSize,$preserved.reason) -ForegroundColor DarkCyan}}
                $targetSizes|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $script:Run 'dry-run-target-sizes.json') -Encoding UTF8
                $modelTargetBytes=[long]5865044386
                $modelRoot=Join-Path $cacheRoot 'models'
                $purgesModelRoot=@($targets|Where-Object{[IO.Path]::GetFullPath($_).TrimEnd('\') -ieq [IO.Path]::GetFullPath($modelRoot).TrimEnd('\')}).Count -gt 0
                if($purgesModelRoot){$downloadImpact=('Se espera redescargar seis modelos: aproximadamente 5.86 GB ({0} bytes).' -f $modelTargetBytes)}else{$downloadImpact='Los dry-runs no incluyen la raíz de modelos; no se estima redescarga de modelos.'}
                $cleanupSummary=@(@($script:CleanupTargetSpecs|Where-Object{$_})|ForEach-Object{$pathText=[string]$_.path;$bytes=[long]$_.bytes;('{0}: {1} bytes' -f $pathText,$bytes)}) -join '; '
                $uninstallSummary=@(@($script:UninstallTargetSpecs|Where-Object{$_})|ForEach-Object{$pathText=[string]$_.path;$bytes=[long]$_.bytes;('{0}: {1} bytes' -f $pathText,$bytes)}) -join '; '
                $residueSummary=@(@($residue|Where-Object{$_})) -join ', '
                $cleanupPreserved=@(@($dry.Cleanup.preserved|Where-Object{$_})|ForEach-Object{'{0}: {1} bytes; {2}' -f $_.path,(Get-DirectorySize $_.path),$_.reason}) -join '; '
                $uninstallPreserved=@(@($dry.Uninstall.preserved|Where-Object{$_})|ForEach-Object{'{0}: {1} bytes; {2}' -f $_.path,(Get-DirectorySize $_.path),$_.reason}) -join '; '
                $cleanupImpact='Operación cleanup. Destinos: {0}. Preservados: {1}. Residuos: {2}. {3}' -f $cleanupSummary,$cleanupPreserved,$residueSummary,$downloadImpact
                $uninstallImpact='Operación uninstall. Destinos: {0}. Preservados: {1}. Residuos: {2}. {3}' -f $uninstallSummary,$uninstallPreserved,$residueSummary,$downloadImpact
                Save-Event @{type='impact';operation='cleanup';targets=$cleanupTargets;impact=$cleanupImpact};Write-Host ('IMPACTO: {0}' -f $cleanupImpact) -ForegroundColor Yellow
                Save-Event @{type='impact';operation='uninstall-preliminar';targets=$uninstallTargets;impact=$uninstallImpact};Write-Host ('IMPACTO: {0}' -f $uninstallImpact) -ForegroundColor Yellow
                $daemonStatus=Invoke-Cli @('daemon','status') 'daemon-before-cleanup' -Json
                if($daemonStatus.Payload.daemon -eq 'running'){$stop=Invoke-Cli @('daemon','stop') 'daemon-stop-before-cleanup' -TimeoutMs 30000 -Json;Assert-True ($stop.Payload.daemon_fully_stopped -eq $true) 'Stop antes de cleanup no confirmó daemon_fully_stopped.';$stopped=Invoke-Cli @('daemon','status') 'daemon-status-before-cleanup' -Json;Assert-True ($stopped.Payload.daemon -eq 'stopped') 'No se pudo verificar daemon stopped antes de cleanup.'}elseif($daemonStatus.Payload.daemon -ne 'stopped'){throw 'BLOCKED: el daemon no está stopped ni running verificable antes de cleanup.'}
                if(@($cleanupTargets).Count){
                $cleanup=Invoke-Cli @('cleanup','--all','--yes') 'cleanup-all' -Json
                foreach($path in @($cleanup.Payload.unreadable)){if($path){throw ('BLOCKED: cleanup dejó ruta ilegible: {0}' -f $path)}}
                $actualPlan=@($cleanup.Payload.planned)
                Assert-True ($actualPlan.Count -eq $cleanupTargets.Count) 'El plan cleanup efectivo difiere del dry-run; se detiene sin continuar con uninstall.'
                foreach($path in $actualPlan){$spec=$script:CleanupTargetSpecs|Where-Object path -eq ([string]$path)|Select-Object -First 1;Assert-True ($null -ne $spec) ('cleanup añadió destino ausente del dry-run: {0}' -f $path)}
                Assert-TargetSetsEqual 'cleanup efectivo' $cleanupTargets $actualPlan
                Assert-True ($cleanup.Payload.status -eq 'cleanup_complete') 'cleanup --all no completó.'
                if(@($cleanup.Payload.unreadable).Count){$unreadable=(@($cleanup.Payload.unreadable)-join ', ');throw ('BLOCKED: cleanup reporta rutas ilegibles: {0}' -f $unreadable)}
                foreach($entry in @($cleanup.Payload.removed)){
                    if($entry -is [string]){$target=$entry}elseif($entry.PSObject.Properties['path']){$target=[string]$entry.path}else{continue}
                    if(Test-Path -LiteralPath $target){throw ('cleanup dejó el destino: {0}' -f $target)}
                }
                }
                $uninstallDry=Get-DryRunTargets
                $script:UninstallTargetSpecs=@(@($uninstallDry.Uninstall.planned)|Where-Object{$_ -and -not [string]::IsNullOrWhiteSpace($_)}|ForEach-Object{[pscustomobject]@{path=[string]$_;bytes=(Get-DirectorySize ([string]$_))}})
                $uninstallTargets=@(Normalize-TargetSet @(@($script:UninstallTargetSpecs|Where-Object{$_})|ForEach-Object path))
                if(@($uninstallTargets).Count){
                $uninstallSummary=@(@($script:UninstallTargetSpecs|Where-Object{$_})|ForEach-Object{'{0}: {1} bytes' -f $_.path,$_.bytes}) -join '; '
                $uninstallPreserved=@(@($uninstallDry.Uninstall.preserved|Where-Object{$_})|ForEach-Object{'{0}: {1} bytes; {2}' -f $_.path,(Get-DirectorySize $_.path),$_.reason}) -join '; '
                $uninstallImpact='Operación uninstall después de cleanup. Destinos: {0}. Preservados: {1}. {2}' -f $uninstallSummary,$uninstallPreserved,$downloadImpact
                Save-Event @{type='impact';operation='uninstall';targets=$uninstallTargets;impact=$uninstallImpact};Write-Host ('IMPACTO: {0}' -f $uninstallImpact) -ForegroundColor Yellow
                $uninstall=Invoke-Cli @('self','uninstall','--yes') 'uninstall' -Json -ExpectedExit @(0,22)
                $uninstallPlan=@($uninstall.Payload.planned)
                $expectedUninstall=$uninstallTargets
                if($uninstall.Payload.status -eq 'not_installed' -and !$uninstallPlan.Count -and !$expectedUninstall.Count){$expectedUninstall=@()}
                Assert-TargetSetsEqual 'uninstall efectivo' $expectedUninstall $uninstallPlan
                Assert-True ($uninstallPlan.Count -eq $expectedUninstall.Count) 'El plan uninstall efectivo difiere del dry-run.'
                foreach($path in $uninstallPlan){$spec=$script:UninstallTargetSpecs|Where-Object path -eq ([string]$path)|Select-Object -First 1;Assert-True ($null -ne $spec) ('uninstall añadió destino ausente del dry-run: {0}' -f $path)}
                Assert-True ($uninstall.Payload.status -in @('uninstalled','removal_scheduled','not_installed')) 'Estado uninstall desconocido.'
                if($uninstall.Payload.status -eq 'removal_scheduled'){
                    $scheduled=@($uninstall.Payload.planned|Where-Object{$_ -eq $programDir})
                    Assert-True ($scheduled.Count -gt 0) 'removal_scheduled no identifica el directorio de programa esperado.'
                    foreach($path in $scheduled){
                        $spec=$script:UninstallTargetSpecs|Where-Object path -eq ([string]$path)|Select-Object -First 1
                        Assert-True ($null -ne $spec) 'El destino removal_scheduled no estaba incluido en el dry-run.'
                    }
                }
                $deadline=[DateTime]::UtcNow.AddMinutes(3)
                $removalChecks=@($ownedPaths+$uninstallPlan|Select-Object -Unique)
                do{
                    $left=@($removalChecks|Where-Object{Test-Path -LiteralPath $_})
                    if(!$left.Count){break}
                    if([DateTime]::UtcNow-ge $deadline){$remainingText=$left -join ', ';throw ('BLOCKED: destinos aún presentes: {0}' -f $remainingText)}
                    Start-Sleep -Seconds 2
                }while($true)
                if($uninstall.Payload.status -eq 'removal_scheduled' -and $left.Count){$remainingText=$left -join ', ';throw ('BLOCKED: removal_scheduled sigue pendiente: {0}' -f $remainingText)}
                if($uninstall.Payload.status -eq 'not_installed' -and $left.Count){$remainingText=$left -join ', ';throw ('BLOCKED: not_installed no explica destinos propios aún presentes: {0}' -f $remainingText)}
                $remainingPath=@(Get-PathEntries|Where-Object{$_.TrimEnd('\') -ieq $programDir.TrimEnd('\')});Assert-True (!$remainingPath.Count) 'PATH no se retiró tras uninstall.'
                $where=Invoke-WhereCheck -ExpectPresent:$false;Assert-True ($where.ExitCode -eq 0) 'where.exe aún encuentra el binario.'
                }
            }
        }else{Set-Step '03-cleanup-empty' 'PASS' 'No había residuos, integración PATH ni binario.'}
    }

    Invoke-Step '04-bootstrap' {
        if(Test-Path -LiteralPath $script:Bin){throw '03-cleanup no dejó la instalación ausente; no se sobrescribe.'}
        $uri='https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v{0}/install.ps1' -f $script:Version
        $bootstrap=Invoke-RestMethod -Uri $uri -TimeoutSec 60
        $bootstrapPath = Join-Path $script:Run 'published-install.ps1'
        $bootstrap | Set-Content -LiteralPath $bootstrapPath -Encoding UTF8
        $hostExe = Join-Path $PSHOME $(if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh.exe' } else { 'powershell.exe' })
        # Sin -PersistedPath a propósito: el hijo conserva el entorno heredado para ejercitar la defensa propia del instalador publicado ante un PSModulePath contaminado.
        $result = Invoke-LoggedProcess -FilePath $hostExe -ArgumentList @('-NoProfile','-NonInteractive','-File',$bootstrapPath) -Name 'published-bootstrap' -TimeoutMs 600000 -Environment @{AVI_NO_SETUP='1'} -SanitizeOutput:$false
        Assert-True ($result.ExitCode -eq 0) 'Bootstrap publicado falló.'
        $receipt=Get-RegisteredRoots;Assert-True ($receipt.version -eq $script:Version) 'Versión del receipt no coincide.'
        $entries=@(Get-PathEntries|Where-Object{$_.TrimEnd('\') -ieq $script:ProgramDir.TrimEnd('\')});Assert-True ($entries.Count -eq 1) 'HKCU PATH no tiene exactamente una entrada.'
        $where=Invoke-WhereCheck -ExpectPresent:$true -AllowPreSetup;Assert-True ($where.ExitCode -eq 0) 'where.exe con PATH persistido falló.'
        $version=Invoke-Cli @('version') 'version-installed' -Json;Assert-PayloadSchema $version.Payload;Assert-True ($version.Payload.version -eq $script:Version) 'Versión instalada inesperada.'
        Assert-True ($version.Payload.schema_version -eq '5') 'El envelope de version no usa schema 5.'
        $bootstrapLog=Get-Content -LiteralPath $result.StdoutPath -Raw
        Assert-True ($bootstrapLog -match '(?i)checksum verificado\.') 'Bootstrap no registra la verificación de checksum del bundle.'
        $zipAsset='ai-voice-interconnector-{0}-x86_64-windows.zip' -f $script:Version
        $zipUri='https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v{0}/{1}' -f $script:Version,$zipAsset
        $zipPath=Join-Path $script:Run $zipAsset
        $zipMetadata=$script:Release.assets|Where-Object name -eq $zipAsset|Select-Object -First 1
        Assert-True ($zipMetadata -and [long]$zipMetadata.size -gt 0) 'Metadata del ZIP publicado no incluye tamaño válido.'
        $zipImpact='Se descargará una copia independiente de {0} ({1} bytes) para recalcular SHA256 contra el mismo asset/versionado del release.' -f $zipAsset,$zipMetadata.size
        Save-Event @{type='impact';operation='zip-download';targets=@($zipPath);impact=$zipImpact};Write-Host ('IMPACTO: {0}' -f $zipImpact) -ForegroundColor Yellow
        Invoke-WebRequest -Uri $zipUri -OutFile $zipPath -TimeoutSec 600
        $actualHash=(Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
        Assert-True ($actualHash -ieq $script:ExpectedWindowsSha256) 'El SHA256 calculado del bundle Windows difiere del SHA256SUMS del release 01-release.'
        Assert-True ((Get-Item -LiteralPath $zipPath).Length -eq [long]$zipMetadata.size) 'El tamaño del ZIP descargado no coincide con el asset del release.'
        Save-Event @{type='checksum';tag=('v'+$script:Version);asset=$zipAsset;expected=$script:ExpectedWindowsSha256;actual=$actualHash;size=(Get-Item -LiteralPath $zipPath).Length;verified=$true}
        Save-Event @{type='bootstrap_source';path=(Join-Path $script:Run 'published-install.ps1');source='release asset';version=$script:Version}
    }

    Invoke-Step '05-setup' {
        $before=Invoke-Cli @('doctor') 'doctor-before-setup' -Json -ExpectedExit @(1)
        Assert-True (@($before.Payload.failed) -contains 'models_provisioned') 'Doctor pre-setup no reporta modelos ausentes.'
        $missing=@($before.Payload.models.missing)
        if($missing.Count -ne 6){throw ('Doctor pre-setup debe identificar seis modelos faltantes; encontró {0}. Se detiene antes de setup.' -f $missing.Count)}
        $pinBytes=@{'qwen3-tts-0.6b'=2498388392;'qwen3-tts-0.6b-base'=2516106051;'parakeet-tdt-v3'=670619706;'opus-mt-es-en'=82536565;'opus-mt-en-es'=82536565;'dpdfnet8'=14857107}
        $estimate=[long]0;foreach($model in $missing){$estimate+=[long]$pinBytes[[string]$model]}
        $estimateGiB=[Math]::Round($estimate/1GB,2)
        $modelImpact='Descarga estimada: {0} bytes ({1} GiB) para los modelos faltantes: {2}.' -f $estimate,$estimateGiB,($missing -join ', ')
        Save-Event @{type='impact';operation='model-download';targets=@($script:ModelsDir);impact=$modelImpact};Write-Host ('IMPACTO: {0}' -f $modelImpact) -ForegroundColor Yellow
        $setup=Invoke-Cli @('setup') 'setup' -TimeoutMs 1200000 -Json -KillOnTimeout
        Assert-PayloadSchema $setup.Payload;Assert-True ($setup.Payload.status -eq 'completed' -and @($setup.Payload.models_provisioned).Count -eq 6) 'Setup no completó los seis modelos.'
        $doctor=Invoke-Cli @('doctor') 'doctor-after-setup' -Json -PersistedPath;Assert-PayloadSchema $doctor.Payload
        Assert-True (@($doctor.Payload.failed).Count -eq 0) 'Doctor reporta fallos.'
        $expectedModels=@('qwen3-tts-0.6b','qwen3-tts-0.6b-base','parakeet-tdt-v3','opus-mt-es-en','opus-mt-en-es','dpdfnet8')
        Assert-True ((@($setup.Payload.models_provisioned|Sort-Object)-join ',') -eq (@($expectedModels|Sort-Object)-join ',')) 'Setup no reporta exactamente los seis modelos fijados.'
        $pathCheck=Invoke-WhereCheck -ExpectPresent:$true;Assert-True ($pathCheck.ExitCode -eq 0) 'where.exe/doctor integrado con PATH persistido falló tras setup.'
        foreach($key in @('parked','stagings','scratch','temporaries','failure_logs','unknown_owner','pending_failures','unreadable')){Assert-True (@($doctor.Payload.pending.$key).Count -eq 0) ('Doctor reporta pending {0}.' -f $key)}
        Assert-True ($doctor.Payload.pending.transaction_journal -eq $false) 'Doctor reporta diario de transacción pendiente.'
        Assert-True ((@($doctor.Payload.models.provisioned|Sort-Object)-join ',') -eq (@($missing|Sort-Object)-join ',')) 'Doctor y setup difieren en los modelos reportados como provisionados.'
        $devices=Invoke-Cli @('devices') 'devices' -Json;Assert-PayloadSchema $devices.Payload;Assert-True ($devices.Payload.PSObject.Properties['devices']) 'devices no devolvió su colección de salida.';$script:Devices=@($devices.Payload.devices);$script:OutputAvailable=($script:Devices.Count -gt 0);$script:InputAvailable=$false
        Save-Event @{type='audio_capability';output_available=$script:OutputAvailable;output_devices=$script:Devices;input_available='not_exposed_by_devices_contract'}
        $receipt=Get-RegisteredRoots;$script:DataDir=[string]$receipt.roots.data_dir;$script:CacheDir=[string]$receipt.roots.cache_dir;$script:ModelsDir=Join-Path $script:CacheDir 'models'
        Assert-True ($doctor.Payload.models.root -eq $script:ModelsDir) 'Doctor no está verificando la raíz de modelos del receipt.'
        $modelSpecs=@{
            'qwen3-tts-0.6b'=@{repo='Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice';revision='85e237c12c027371202489a0ec509ded67b5e4b5';files=@('config.json','generation_config.json','merges.txt','model.safetensors','preprocessor_config.json','speech_tokenizer\config.json','speech_tokenizer\configuration.json','speech_tokenizer\model.safetensors','speech_tokenizer\preprocessor_config.json','tokenizer_config.json','vocab.json')}
            'qwen3-tts-0.6b-base'=@{repo='Qwen/Qwen3-TTS-12Hz-0.6B-Base';revision='5d83992436eae1d760afd27aff78a71d676296fc';files=@('config.json','generation_config.json','merges.txt','model.safetensors','preprocessor_config.json','speech_tokenizer\config.json','speech_tokenizer\configuration.json','speech_tokenizer\model.safetensors','speech_tokenizer\preprocessor_config.json','tokenizer_config.json','vocab.json')}
            'parakeet-tdt-v3'=@{repo='istupakov/parakeet-tdt-0.6b-v3-onnx';revision='8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce';files=@('encoder-model.int8.onnx','decoder_joint-model.int8.onnx','nemo128.onnx','vocab.txt')}
            'opus-mt-es-en'=@{repo='CristianRojaas/opus-mt-es-en-ct2-int8';revision='6eabf2f7d9f92dd52e38fe95c9da29f219613c19';files=@('config.json','model.bin','shared_vocabulary.json','source.spm','target.spm')}
            'opus-mt-en-es'=@{repo='CristianRojaas/opus-mt-en-es-ct2-int8';revision='452971fec59e5a4093f8630e55022909dc5beceb';files=@('config.json','model.bin','shared_vocabulary.json','source.spm','target.spm')}
            'dpdfnet8'=@{repo='Ceva-IP/DPDFNet';revision='dd6818d00f50c836fed43a6243ebe49116de5964';files=@('onnx\dpdfnet8_48khz_hr.onnx')}
        }
        foreach($name in $modelSpecs.Keys){$spec=$modelSpecs[$name];$null=Get-ModelSnapshot $name $spec.repo $spec.revision $spec.files}
        Save-Event @{type='setup';models=$setup.Payload.models_provisioned;model_root=$script:ModelsDir;receipt=$receipt}
    }

    Invoke-Step '06-daemon' {
        $status=Invoke-Cli @('daemon','status') 'daemon-initial' -Json;Assert-PayloadSchema $status.Payload;Assert-True ($status.Payload.daemon -eq 'stopped') 'Daemon inicialmente debía estar stopped.'
        Start-ManagedDaemon
        $start=Invoke-Cli @('daemon','start') 'daemon-start-idempotent' -Json
        Assert-True ($start.Payload.status -eq 'already_running' -and $start.Payload.daemon -eq 'running' -and $start.Payload.pid -eq $script:DaemonPid) 'daemon start no confirma el PID recién iniciado y running.'
    }

    Invoke-Step '07-voices' {
        $voices=Invoke-Cli @('voice','list') 'voice-list' -Json
        foreach($name in @('default','ryan','vivian')){if(@($voices.Payload.voices) -notcontains $name){throw ('Voz factory ausente: {0}' -f $name)}}
        $clone=Invoke-VoiceClone 'mi_voz';Assert-PayloadSchema $clone.Payload;Assert-True ($clone.Payload.precomputed -eq $false) 'Clone directo debía ser precomputed false.'
        $qvoice=$clone.QvoicePath;Assert-True (Test-Path -LiteralPath $qvoice -PathType Leaf) 'La ruta speech del clone no existe.';Assert-True ((Get-Item -LiteralPath $qvoice).Length -gt 1MB) 'qvoice debe superar 1 MB.';Assert-True (!(Test-Path -LiteralPath (Join-Path (Split-Path -Parent $qvoice) 'speech-reference.wav'))) 'El WAV fuente fue copiado.'
        $cloneDaemon=Invoke-VoiceClone 'mi_voz_daemon' -Daemon;Assert-PayloadSchema $cloneDaemon.Payload;Assert-True ((Get-Item -LiteralPath $cloneDaemon.QvoicePath).Length -gt 0) 'qvoice daemon vacío.'
        $conflict=Invoke-VoiceClone 'mi_voz' -ExpectedExit @(6);Assert-True ($conflict.Payload.reason -in @('voice_exists','state_conflict')) 'Conflicto de voz no emitió reason esperado.'
        $forced=Invoke-VoiceClone 'mi_voz' -Force;Assert-True ($forced.ExitCode -eq 0 -and $forced.Payload.name -eq 'mi_voz' -and $forced.Payload.precomputed -eq $false) 'Clone --force no devolvió el resultado esperado.'
        $denoiseClone=Invoke-VoiceClone 'mi_voz_denoise' -Denoise;Assert-True ((Get-Item -LiteralPath (Get-VoiceQvoice 'mi_voz_denoise')).Length -gt 0) 'qvoice denoise vacío.'
        if($script:InputAvailable){$mic=Invoke-VoiceClone 'mi_voz_mic' -Mic;Assert-True ($mic.ExitCode -eq 0) 'Clone mic falló.'}else{Set-SubcaseResult '07-voices-mic' 'SKIP' -Reason 'devices --json enumera solo salidas y la CLI no expone un probe de entrada.';Set-Step '07-voices-mic' 'SKIP' 'No hay evidencia de dispositivo de entrada; la CLI no publica inventario de captura y no se invoca el micrófono.' -Reason 'devices --json enumera solo salidas; capability de captura no expuesta.'}
    }

    Invoke-Step '08-synthesis' {
        $auto=Invoke-Cli @('speech','synthesize','--text','Hola mundo','--label','e2e_auto_default','--voice','default') 'synth-auto' -Json
        Assert-PayloadSchema $auto.Payload;Assert-True ($auto.Payload.status -eq 'success') 'Auto synthesis no tuvo status success.'
        $script:WavShortDefault=[IO.Path]::GetFullPath([string]$auto.Payload.audio_path);Assert-Wav $script:WavShortDefault;$script:WavRetained=$script:WavShortDefault
        $forcedDaemon=Invoke-Cli @('speech','synthesize','--text','Hola mundo','--label','e2e_daemon_default','--voice','default','--daemon') 'synth-force-daemon' -Json
        Assert-True ($forcedDaemon.Payload.status -eq 'success') 'ForceDaemon synthesis falló.';Assert-Wav ([IO.Path]::GetFullPath([string]$forcedDaemon.Payload.audio_path))
        $autoClone=Invoke-Cli @('speech','synthesize','--text','Hola clon','--label','e2e_auto_clone','--voice','mi_voz') 'synth-auto-clone' -Json
        Assert-True ($autoClone.Payload.status -eq 'success') 'Synthesis Auto de voz clonada falló.'
        Assert-Wav ([IO.Path]::GetFullPath([string]$autoClone.Payload.audio_path));$wavAutoClone=[IO.Path]::GetFullPath([string]$autoClone.Payload.audio_path)
        $cloneDaemonWav=Invoke-Cli @('speech','synthesize','--text','Hola mundo','--label','e2e_daemon_clone','--voice','mi_voz','--daemon') 'synth-clone-daemon-wer' -Json
        Assert-True ($cloneDaemonWav.Payload.status -eq 'success') 'Síntesis daemon de voz clonada falló.'
        $script:WavShortClone=[IO.Path]::GetFullPath([string]$cloneDaemonWav.Payload.audio_path)
        Assert-Wav $script:WavShortClone
        if($script:OutputAvailable){$sayClone=Invoke-Cli @('speech','say','--text','Hola clon','--voice','mi_voz','--no-daemon') 'say-clone' -Json;Assert-True ($sayClone.Payload.status -eq 'reproduced') 'say cloned no reporta reproduced.';$sayFactory=Invoke-Cli @('speech','say','--text','Hola factory','--voice','default','--daemon') 'say-factory' -Json;Assert-True ($sayFactory.Payload.status -eq 'reproduced') 'say factory no reporta reproduced.'}else{Set-SubcaseResult '08-synthesis-say' 'SKIP' -Reason 'No se enumeró salida de audio; say no se invocó.';Set-Step '08-synthesis-say' 'SKIP' 'No se observó dispositivo de salida.'}
        Stop-ManagedDaemon
        $direct=Invoke-Cli @('speech','synthesize','--text','Hello world','--label','e2e_direct_en','--voice','default','--source-language','en','--target-language','en','--temperature','0.7','--no-daemon') 'synth-force-direct' -Json;Assert-True ($direct.Payload.status -eq 'success') 'ForceDirect synthesis falló.';Assert-Wav ([IO.Path]::GetFullPath([string]$direct.Payload.audio_path))
        $directClone=Invoke-Cli @('speech','synthesize','--text','Hola clon','--label','e2e_direct_clone','--voice','mi_voz','--no-daemon') 'synth-force-direct-clone' -Json;Assert-True ($directClone.Payload.status -eq 'success') 'ForceDirect synthesis de voz clonada falló.';Assert-Wav ([IO.Path]::GetFullPath([string]$directClone.Payload.audio_path))
        $cross=Invoke-Cli @('speech','synthesize','--text','Hola mundo','--label','e2e_direct_cross','--voice','default','--source-language','es','--target-language','en','--temperature','0.7','--no-daemon') 'synth-direct-cross' -Json;Assert-True ($cross.Payload.status -eq 'success') 'Síntesis directa cross-language falló.';Assert-Wav ([IO.Path]::GetFullPath([string]$cross.Payload.audio_path))
        Assert-Wav $script:WavShortClone
        $nativeStt=Test-NativeSttCapability -AudioPath (Join-Path $script:Root 'crates\avi-stt\tests\assets\parakeet_sample_16k.wav')
        $parakeetSpec=@{repo='istupakov/parakeet-tdt-0.6b-v3-onnx';revision='8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce';files=@('nemo128.onnx')}
        $parakeetPath=Join-Path $script:ModelsDir ('models--istupakov--parakeet-tdt-0.6b-v3-onnx\snapshots\'+$parakeetSpec.revision+'\nemo128.onnx')
        $parakeetItem=Get-Item -LiteralPath $parakeetPath -ErrorAction SilentlyContinue
        $parakeetAvailable=($null -ne $parakeetItem -and $parakeetItem.Length -gt 0)
        if($nativeStt -and $parakeetAvailable){
            foreach($sample in @(@{name='default';path=$script:WavShortDefault;text='Hola mundo'},@{name='clone';path=$script:WavShortClone;text='Hola mundo'})){$stt=Invoke-Cli @('speech','transcribe','--audio',$sample.path,'--source-language','es','--no-daemon') ('08-synthesis-wer-'+$sample.name) -Json;Assert-PayloadSchema $stt.Payload;Assert-True ($stt.Payload.source_language -eq 'es' -and ![string]::IsNullOrWhiteSpace($stt.Payload.text)) 'STT WER devolvió vacío o idioma de origen incorrecto.';$wer=Get-Wer $sample.text $stt.Payload.text;$result=[pscustomobject]@{reference=$sample.text;transcript=$stt.Payload.text;wer=$wer;audio=$sample.path};$script:WerResults.Add($result);Assert-True ($wer -le 0.25) ('WER {0} supera 0.25.' -f $wer);Set-SubcaseResult ('08-synthesis-wer-'+$sample.name) 'PASS' -Command ($script:Bin+' speech transcribe --audio '+$sample.path) -ExitCode $stt.ExitCode -Payload $result -Evidence @($stt.StdoutPath,$stt.StderrPath);Save-Event @{type='wer';reference=$sample.text;transcript=$stt.Payload.text;wer=$wer;audio=$sample.path}}
        }else{foreach($case in @('08-synthesis-wer-default','08-synthesis-wer-clone')){Set-SubcaseResult $case 'SKIP' -Reason ('Capacidad ausente antes de transcribir: native-stt={0}, parakeet_model={1}.' -f $nativeStt,$parakeetAvailable);Set-Step $case 'SKIP' 'Feature o modelo Parakeet no disponible antes de WER.' -Reason ('native-stt={0}; parakeet_model={1}' -f $nativeStt,$parakeetAvailable)}}
    }

    Invoke-Step '09-store' {
        $list=Invoke-Cli @('speech','list') 'speech-list' -Json;Assert-True (@($list.Payload.speech|ForEach-Object label) -contains 'e2e_auto_default') 'Speech store no contiene síntesis.'
        foreach($label in @('e2e_auto_default','e2e_daemon_default','e2e_auto_clone','e2e_daemon_clone','e2e_direct_en','e2e_direct_clone','e2e_direct_cross')){if(@($list.Payload.speech|ForEach-Object label) -contains $label){Set-SubcaseResult ('09-store-label-'+$label) 'PASS' -Command ($script:Bin+' speech list --json') -ExitCode $list.ExitCode -Payload @{label=$label;present=$true} -Evidence @($list.StdoutPath,$list.StderrPath)}else{Set-SubcaseResult ('09-store-label-'+$label) 'FAIL' -Command ($script:Bin+' speech list --json') -ExitCode $list.ExitCode -Payload @{label=$label;present=$false} -Evidence @($list.StdoutPath,$list.StderrPath);throw ('Speech store no contiene label requerido: {0}' -f $label)}}
        Assert-True (@($list.Payload.speech|ForEach-Object label) -contains 'e2e_direct_cross') 'Speech store no contiene síntesis directa cross-language.'
        Assert-True (@($list.Payload.speech|ForEach-Object label) -contains 'e2e_direct_en') 'Speech store no contiene síntesis directa del caso temperatura.'
        Assert-True (@($list.Payload.speech|ForEach-Object label) -contains 'e2e_direct_clone') 'Speech store no contiene síntesis ForceDirect de voz clonada.'
        $cloneList=Invoke-Cli @('speech','list','--voice','mi_voz') 'speech-list-clone' -Json;Assert-True (@($cloneList.Payload.speech|ForEach-Object label) -contains 'e2e_auto_clone') 'Speech store no conserva síntesis Auto de voz clonada.'
        $filtered=Invoke-Cli @('speech','list','--voice','mi_voz') 'speech-list-filter' -Json;Assert-True (@($filtered.Payload.speech|ForEach-Object label) -contains 'e2e_auto_clone') 'Filtro por voz no devuelve su etiqueta de síntesis.'
        if($script:OutputAvailable){$play=Invoke-Cli @('speech','play','--label','e2e_auto_default','--voice','default') 'speech-play' -Json;Assert-True ($play.Payload.status -eq 'played') 'Speech play no reportó played.'}else{Set-SubcaseResult '09-store-play' 'SKIP' -Reason 'No se enumeró salida de audio; play no se invocó.';Set-Step '09-store-play' 'SKIP' 'No hay salida de audio.'}
        $removeSpeech=Invoke-Cli @('speech','remove','--label','e2e_direct_en','--voice','default') 'speech-remove' -Json
        Assert-True ($removeSpeech.Payload.status -eq 'removed' -and $removeSpeech.Payload.label -eq 'e2e_direct_en') 'Speech remove no confirmó la locución esperada.'
        $speechAfterRemove=Invoke-Cli @('speech','list') 'speech-list-after-remove' -Json;Assert-True (@($speechAfterRemove.Payload.speech|ForEach-Object label) -notcontains 'e2e_direct_en') 'La etiqueta removida continúa en speech store.'
        $null=Invoke-Cli @('speech','synthesize','--text','Test voice','--voice','mi_voz_denoise','--label','e2e_voice_remove','--no-daemon') 'synth-disposable' -Json
        $removeVoice=Invoke-Cli @('voice','remove','--name','mi_voz_denoise','--yes') 'voice-remove-disposable' -Json
        Assert-True ($removeVoice.Payload.status -eq 'removed' -and $removeVoice.Payload.voice -eq 'mi_voz_denoise' -and @($removeVoice.Payload.removed_speech).Count -ge 1) 'Voice remove no confirmó la voz y su namespace desechable.'
        $afterVoices=Invoke-Cli @('voice','list') 'voice-list-after-remove' -Json;Assert-True (@($afterVoices.Payload.voices) -notcontains 'mi_voz_denoise') 'La voz desechable sigue presente.'
    }

    Invoke-Step '10-stt' {
        $parakeetPath=Join-Path $script:ModelsDir 'models--istupakov--parakeet-tdt-0.6b-v3-onnx\snapshots\8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce\nemo128.onnx'
        $parakeetFile=Get-Item -LiteralPath $parakeetPath -ErrorAction SilentlyContinue
        $nativeStt=$false
        if($parakeetFile -and $parakeetFile.Length -gt 0){$nativeStt=Test-NativeSttCapability -AudioPath $sttSample}
        if($nativeStt -and $parakeetFile -and $parakeetFile.Length -gt 0){
            $stt=Invoke-Cli @('speech','transcribe','--audio',$sttSample,'--source-language','es','--no-daemon') 'stt-audio' -Json;Assert-PayloadSchema $stt.Payload;Assert-True (![string]::IsNullOrWhiteSpace($stt.Payload.text) -and $stt.Payload.source_language -eq 'es') 'STT sample vacío o source_language incorrecto.'
            Set-SubcaseResult '10-stt-audio' 'PASS' -Command ($script:Bin+' speech transcribe --audio '+$sttSample) -ExitCode $stt.ExitCode -Payload $stt.Payload -Evidence @($stt.StdoutPath,$stt.StderrPath);Set-Step '10-stt-audio' 'PASS' 'Transcripción de audio validada.' -Command ($script:Bin+' speech transcribe --audio '+$sttSample) -ExitCode $stt.ExitCode -Payload $stt.Payload -Evidence @($stt.StdoutPath,$stt.StderrPath)
        }else{Set-SubcaseResult '10-stt-audio' 'SKIP' -Reason 'native-stt o Parakeet no estaba disponible antes de intentar STT.';Set-Step '10-stt-audio' 'SKIP' 'native-stt o Parakeet ausente antes de transcribir.' -Reason 'Capacidad opcional no provisionada.'}
        if($script:InputAvailable){$mic=Invoke-Cli @('speech','transcribe','--mic','--duration','5','--source-language','es','--no-daemon') 'stt-mic' -Json;Assert-PayloadSchema $mic.Payload;Assert-True (![string]::IsNullOrWhiteSpace($mic.Payload.text) -and $mic.Payload.source_language -eq 'es') 'STT mic vacío o source_language incorrecto.'}else{Set-SubcaseResult '10-stt-mic' 'SKIP' -Reason 'La CLI no expone comprobación independiente de entrada y no se invocó micrófono.';Set-Step '10-stt-mic' 'SKIP' 'No hay evidencia de dispositivo de entrada; la CLI no publica inventario de captura y no se invoca el micrófono.' -Reason 'devices --json enumera solo salidas; capability de captura no expuesta.'}
    }

    Invoke-Step '11-translate' {
        Start-ManagedDaemon
        $tr=Invoke-Cli @('translate','--text','Hola','--source-language','es','--target-language','en','--daemon') 'translate-es-en' -Json;Assert-True ($tr.Payload.translated -and $tr.Payload.translated -ne 'Hola') 'Translate es-en falló.'
        Stop-ManagedDaemon
        $tr=Invoke-Cli @('translate','--text','Hello','--source-language','en','--target-language','es','--no-daemon') 'translate-en-es' -Json;Assert-True ($tr.Payload.translated -and $tr.Payload.translated -ne 'Hello') 'Translate en-es falló.'
        $pass=Invoke-Cli @('translate','--text','Hola','--source-language','es','--target-language','es','--no-daemon') 'translate-passthrough' -Json;Assert-True ($pass.Payload.translated -eq 'Hola') 'Passthrough modificó texto.'
    }

    Invoke-Step '12-dub' {
        if($script:OutputAvailable){
            Start-ManagedDaemon
            $dub=Invoke-Cli @('speech','dub','--audio',$sttSample,'--source-language','es','--target-language','en','--daemon') 'dub-daemon-cross' -Json;Assert-True ($dub.Payload.status -eq 'dubbed' -and $dub.Payload.text) 'Dub daemon no produjo resultado.'
            Save-Event @{type='dub';mode='daemon';source='es';target='en';text=$dub.Payload.text;status=$dub.Payload.status}
            $dubCommand=$script:Bin+' '+($dub.Arguments -join ' ');Set-SubcaseResult '12-dub-daemon' 'PASS' -Command $dubCommand -ExitCode $dub.ExitCode -Payload $dub.Payload -Evidence @($dub.StdoutPath,$dub.StderrPath);Set-Step '12-dub-daemon' 'PASS' 'Dub daemon completó con status dubbed y texto no vacío.' -Command $dubCommand -ExitCode $dub.ExitCode -Payload $dub.Payload -Evidence @($dub.StdoutPath,$dub.StderrPath)
            Stop-ManagedDaemon
            $dub=Invoke-Cli @('speech','dub','--audio',$script:WavRetained,'--source-language','es','--target-language','es','--no-daemon') 'dub-direct' -Json;Assert-True ($dub.Payload.status -eq 'dubbed' -and $dub.Payload.text) 'Dub directo no produjo resultado.';Save-Event @{type='dub';mode='direct';source='es';target='es';text=$dub.Payload.text;status=$dub.Payload.status}
            $dubCommand=$script:Bin+' '+($dub.Arguments -join ' ');Set-SubcaseResult '12-dub-direct' 'PASS' -Command $dubCommand -ExitCode $dub.ExitCode -Payload $dub.Payload -Evidence @($dub.StdoutPath,$dub.StderrPath);Set-Step '12-dub-direct' 'PASS' 'Dub directo completó con status dubbed y texto no vacío.' -Command $dubCommand -ExitCode $dub.ExitCode -Payload $dub.Payload -Evidence @($dub.StdoutPath,$dub.StderrPath)
        }else{Set-SubcaseResult '12-dub-daemon' 'SKIP' -Reason 'No hay dispositivo de salida enumerado; dub no se invocó.';Set-Step '12-dub-daemon' 'SKIP' 'No hay salida de audio; ambos casos dub se omiten antes de ejecutarlos.' -Reason 'devices --json válido no enumeró salida.';Set-SubcaseResult '12-dub-direct' 'SKIP' -Reason 'No hay dispositivo de salida enumerado; dub no se invocó.';Set-Step '12-dub-direct' 'SKIP' 'No hay salida de audio; ambos casos dub se omiten antes de ejecutarlos.' -Reason 'devices --json válido no enumeró salida.'}
    }

    Invoke-Step '13-denoise' {
        $dpdfPath=Join-Path $script:ModelsDir ('models--Ceva-IP--DPDFNet\snapshots\dd6818d00f50c836fed43a6243ebe49116de5964\onnx\dpdfnet8_48khz_hr.onnx');Assert-True (Test-Path -LiteralPath $dpdfPath -PathType Leaf) 'DPDFNet no está provisionado para casos denoise.'
        $nativeStt=Test-NativeSttCapability -AudioPath $sttSample
        $parakeetPath=Join-Path $script:ModelsDir 'models--istupakov--parakeet-tdt-0.6b-v3-onnx\snapshots\8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce\nemo128.onnx'
        $parakeetFile=Get-Item -LiteralPath $parakeetPath -ErrorAction SilentlyContinue
        if($nativeStt -and $parakeetFile -and $parakeetFile.Length -gt 0){$denoiseTranscribe=Invoke-Cli @('speech','transcribe','--audio',$sttSample,'--source-language','es','--denoise','--no-daemon') 'stt-denoise' -Json;Assert-PayloadSchema $denoiseTranscribe.Payload;Assert-True ($denoiseTranscribe.Payload.text -and $denoiseTranscribe.Payload.source_language -eq 'es') 'STT denoise vacío o idioma incorrecto.';Save-Event @{type='denoise_opt_in';operation='transcribe';audio=$sttSample;transcript=$denoiseTranscribe.Payload.text}}else{Set-SubcaseResult '13-denoise-stt' 'SKIP' -Reason 'native-stt o Parakeet ausente antes del caso denoise STT.';Set-Step '13-denoise-stt' 'SKIP' 'native-stt o Parakeet ausente antes del caso denoise STT.' -Reason 'Capacidad o modelo opcional ausente.'}
        if($script:OutputAvailable){$denoiseDub=Invoke-Cli @('speech','dub','--audio',$script:WavRetained,'--source-language','es','--target-language','es','--denoise','--no-daemon') 'dub-denoise' -Json;Assert-PayloadSchema $denoiseDub.Payload;Assert-True ($denoiseDub.Payload.status -eq 'dubbed' -and $denoiseDub.Payload.text) 'Dub denoise vacío o sin status dubbed.';Save-Event @{type='denoise_opt_in';operation='dub';audio=$script:WavRetained;transcript=$denoiseDub.Payload.text}}else{Set-SubcaseResult '13-denoise-dub' 'SKIP' -Reason 'No hay salida de audio; dub denoise no se invocó.';Set-Step '13-denoise-dub' 'SKIP' 'No hay salida de audio para dub denoise.' -Reason 'Caso dependiente de salida omitido antes de invocarlo.'}
        $denoiseDir=Join-Path $script:Run 'denoise';[void](New-Item -ItemType Directory -Path $denoiseDir)
        $version=Invoke-Cli @('speech','denoise','--version') 'ffmpeg-version' -Json:$false;Assert-True ($version.Stdout -match '(?i)ffmpeg') 'No se identificó FFmpeg empaquetado.'
        foreach($preset in @('studio','default')){
            $presetDir=Join-Path $denoiseDir $preset
            [void](New-Item -ItemType Directory -Path $presetDir)
            $args=@('speech','denoise',$script:Reference,'--output-dir',$presetDir,'--json')
            if($preset -eq 'default'){$args+=@('--preset','default')}
            $denoiseResult=Invoke-Cli -Arguments $args -Name ('13-denoise-{0}' -f $preset) -Json:$false
            $lines=@($denoiseResult.Stdout -split '\r?\n'|Where-Object{$_.Trim()})
            Assert-True ($lines.Count -ge 2) 'Salida JSONL denoise incompleta.'
            $records=@($lines|ForEach-Object{ConvertFrom-JsonStrict $_})
            $summary=$records|Where-Object{$_.PSObject.Properties['summary']}
            Assert-True ($summary -and $summary.summary.failed -eq 0 -and $summary.summary.ok -gt 0) 'Resumen denoise JSONL reporta fallos o cero salidas.'
            $itemRecords=@($records|Where-Object{$_.PSObject.Properties['output'] -and $_.output})
            Assert-True ($itemRecords.Count -gt 0) 'JSONL denoise no lista salidas.'
            $presetOutputs=@()
            foreach($item in $itemRecords){
                $outputPath=[IO.Path]::GetFullPath([string]$item.output)
                if(!(Test-Path -LiteralPath $outputPath -PathType Leaf)){throw ('No existe la salida JSONL: {0}' -f $outputPath)}
                if((Get-Item -LiteralPath $outputPath).Length -le 0){throw ('Salida JSONL vacía: {0}' -f $outputPath)}
                $presetOutputs+=@($outputPath)
            }
            $script:DenoiseOutputByPreset[$preset]=$presetOutputs
            $script:DenoiseOutput+=@($presetOutputs)
            Set-SubcaseResult ('13-denoise-'+$preset) 'PASS' -Command ($script:Bin+' '+($args -join ' ')) -ExitCode $denoiseResult.ExitCode -Payload $summary.summary -Evidence @($denoiseResult.StdoutPath,$denoiseResult.StderrPath)+$presetOutputs
        }
        $script:DenoiseOutput=@($script:DenoiseOutput|Select-Object -Unique);Assert-True ($script:DenoiseOutput.Count -ge 2) 'No hay salidas no vacías de ambos presets denoise.'
    }

    Invoke-Step '14-final-state' {
        $entries=@(Get-PathEntries|Where-Object{$_.TrimEnd('\') -ieq $script:ProgramDir.TrimEnd('\')});Assert-True ($entries.Count -eq 1) 'PATH no contiene una entrada exacta.'
        $where=Invoke-WhereCheck -ExpectPresent:$true;Assert-True ($where.ExitCode -eq 0) 'where/doctor del hijo con PATH persistido fallaron.'
        $doctor=Invoke-Cli @('doctor') 'doctor-final' -Json -PersistedPath;Assert-PayloadSchema $doctor.Payload;Assert-True (@($doctor.Payload.failed).Count -eq 0) 'Doctor final falló.'
        $pathCheck=Invoke-WhereCheck -ExpectPresent:$true;Assert-True ($pathCheck.ExitCode -eq 0) 'where.exe y doctor no pasaron en el mismo hijo con PATH persistido.'
        Assert-True ($pathCheck.Doctor.install.version -eq $script:Version -and $pathCheck.Doctor.version -eq $script:Version) 'Doctor del mismo hijo informa receipt/binario de versión distinta.'
        Assert-True ($pathCheck.Doctor.install.receipt -eq 'valid' -and $pathCheck.Doctor.path.resolves_to_this_install -eq $true -and @($pathCheck.Doctor.path.duplicate_entries).Count -eq 0) 'Receipt o resolución PATH del mismo hijo no son válidos.'
        Assert-True ((@($script:SubcaseResults|Where-Object{($_.name -eq '08-synthesis-wer-default' -or $_.name -eq '08-synthesis-wer-clone') -and $_.status -eq 'PASS'}).Count -eq 2 -or @($script:SubcaseResults|Where-Object{($_.name -eq '08-synthesis-wer-default' -or $_.name -eq '08-synthesis-wer-clone') -and $_.status -eq 'SKIP'}).Count -eq 2) -and @($script:WerResults|Where-Object wer -gt 0.25).Count -eq 0) 'Los dos WER deben existir y ser ≤ 0.25.'
        Assert-True ($pathCheck.Doctor.install.dir -eq $script:ProgramDir -and $pathCheck.Doctor.path.resolves_to_this_install -eq $true) 'Doctor del mismo hijo informa otra instalación como propietaria/resuelta.'
        Assert-True ($pathCheck.Doctor.models.root -eq $script:ModelsDir -and @($pathCheck.Doctor.models.missing).Count -eq 0 -and @($pathCheck.Doctor.models.provisioned).Count -eq 6) 'Doctor del mismo hijo no confirma la raíz y los seis modelos instalados.'
        $versionFinal=Invoke-Cli @('version') 'version-final' -Json -PersistedPath;Assert-PayloadSchema $versionFinal.Payload;Assert-True ($versionFinal.Payload.version -eq $script:Version) 'version --json final no coincide con el release.'
        $dataSize=Get-DirectorySize $script:DataDir;$modelsSize=Get-DirectorySize $script:ModelsDir
        Save-Event @{type='final_sizes';data_root=$script:DataDir;data_bytes=$dataSize;models_root=$script:ModelsDir;models_bytes=$modelsSize}
        foreach($key in @('parked','stagings','scratch','temporaries','failure_logs','unknown_owner','pending_failures','unreadable')){Assert-True (@($doctor.Payload.pending.$key).Count -eq 0) ('Pending final no vacío: {0}' -f $key)}
        $receipt=Get-RegisteredRoots;Assert-True ($receipt.version -eq $script:Version) 'Receipt final no coincide con versión fuente/release.'
        Assert-True ($doctor.Payload.pending.transaction_journal -eq $false) 'Pending final reporta diario transaccional.'
        foreach($voice in @('mi_voz','mi_voz_daemon')){Assert-True ((Get-Item -LiteralPath (Get-VoiceQvoice $voice)).Length -gt 0) ('qvoice retenido vacío: {0}' -f $voice)}
        foreach($wav in @($script:WavShortDefault,$script:WavShortClone)){Assert-Wav $wav}
        foreach($preset in @('studio','default')){$outputs=@($script:DenoiseOutputByPreset[$preset]);Assert-True ($outputs.Count -gt 0) ('El preset denoise {0} no dejó salidas retenidas.' -f $preset);foreach($path in $outputs){Assert-True ((Test-Path -LiteralPath $path -PathType Leaf) -and (Get-Item -LiteralPath $path).Length -gt 0) ('Salida denoise retenida ausente o vacía: {0}' -f $path)}}
        $status=Invoke-Cli @('daemon','status') 'daemon-final' -Json
        Assert-PayloadSchema $status.Payload
        if($status.Payload.daemon -eq 'running'){$identity=[int]$status.Payload.pid;Assert-True ($script:DaemonStartedIdentities.Contains($identity)) 'Daemon final running no pertenece a un PID iniciado por esta corrida.'}
        Set-Step '14-final-state-daemon' 'PASS' 'Estado final consultado.' -Command ($script:Bin+' daemon status --json') -ExitCode $status.ExitCode -Payload $status.Payload -Evidence @($status.StdoutPath,$status.StderrPath)
        Assert-True ($status.Payload.daemon -in @('running','stopped')) 'Estado daemon final no reconocido.'
        if($status.Payload.daemon -eq 'running'){Assert-True ($status.Payload.warm -eq 'warm') 'Daemon final running sin warm verificado.'}
        $finalWarm = if ($status.Payload.PSObject.Properties['warm']) { $status.Payload.warm } else { $null }
        Save-Event @{type='final';version=$script:Version;receipt=(Get-RegisteredRoots);models=$doctor.Payload.models.provisioned;wav=$script:WavRetained;denoise=$script:DenoiseOutput;daemon=$status.Payload.daemon;warm=$finalWarm}
    }
}

try {
    Initialize-Run
    Run-E2E
    $unresolved=@($script:Results|Where-Object status -in @('PENDING','RUNNING'))
    Assert-True ($unresolved.Count -eq 0) ('Hay resultados sin estado terminal: {0}' -f (@($unresolved|ForEach-Object step)-join ', '))
    if ($script:TotalStopwatch -and $script:TotalStopwatch.IsRunning) { $script:TotalStopwatch.Stop() }
    $totalMs = if ($script:TotalStopwatch) { $script:TotalStopwatch.ElapsedMilliseconds } else { [long]0 }
    $totalFormatted = Format-Duration $totalMs
    $resultDocument=[pscustomobject]@{
        run=$script:Run
        overall='PASS'
        total_duration_ms=$totalMs
        total_duration=$totalFormatted
        steps=@($script:Results)
        subcases=@($script:SubcaseResults)
        events=(Join-Path $script:Run 'events.jsonl')
    }
    Set-Content -LiteralPath (Join-Path $script:Run 'result.json') -Value ($resultDocument|ConvertTo-Json -Depth 30) -Encoding UTF8
    Write-TimingSummary -TotalMs $totalMs
    Write-Result 'PASS' ('E2E completada en {0}. Evidencia: {1}' -f $totalFormatted, $script:Run)
    exit 0
} catch {
    if ($script:TotalStopwatch -and $script:TotalStopwatch.IsRunning) { $script:TotalStopwatch.Stop() }
    $totalMs = if ($script:TotalStopwatch) { $script:TotalStopwatch.ElapsedMilliseconds } else { [long]0 }
    $totalFormatted = Format-Duration $totalMs
    $message=$_.Exception.Message
    $status=if($message.StartsWith('BLOCKED:')){'BLOCKED'}else{'FAIL'}
    if($script:Run){
        foreach($phase in @($script:Phases.Id)){
            if(!@($script:Results|Where-Object step -eq $phase).Count){
                Set-Step $phase 'NOT_RUN' 'No ejecutada por la detención previa.' -Payload @{executed=$false} -Reason $message
            }
        }
        Set-Step 'run' $status $message -Reason $message -DurationMs $totalMs
        $resultDocument=[pscustomobject]@{
            run=$script:Run
            overall=$status
            total_duration_ms=$totalMs
            total_duration=$totalFormatted
            steps=@($script:Results)
            subcases=@($script:SubcaseResults)
            events=(Join-Path $script:Run 'events.jsonl')
        }
        Set-Content -LiteralPath (Join-Path $script:Run 'result.json') -Value ($resultDocument|ConvertTo-Json -Depth 30) -Encoding UTF8
        Write-TimingSummary -TotalMs $totalMs
    } else {
        Write-Result 'BLOCKED' $message
    }
    Write-Result $status $message
    Write-Result 'INFO' 'No se ejecuta rollback automático; se preservan estado parcial y evidencias.'
    exit 1
} finally {
    if($script:DaemonOwned){Write-Result 'BLOCKED' ('Daemon registrado como iniciado por esta corrida; PID {0}. No se mata por nombre ni se altera tras fallo.' -f $script:DaemonPid)}
}
