# Servidor falso local para la suite Pester (solo pruebas, HTTP en 127.0.0.1).
#
# Se ejecuta como proceso hijo (Start-Process -PassThru); sirve el arbol de
# <Root> tal cual (<Root>/vX.Y.Z/<fichero>), responde 404 fuera del arbol y
# registra cada peticion en <RequestLog> para las aserciones de "antes de
# descargar". HTTP plano basta porque el bootstrap de Windows no fija --proto
# como el de POSIX. Los ficheros se sirven como application/octet-stream, sin
# charset, igual que los assets de GitHub: asi `irm` decodifica el cuerpo como
# en produccion (ISO-8859-1 en PowerShell 5.1, UTF-8 en 7).
# Uso: powershell -File Serve.ps1 -Root <dir> -PortFile <f> -RequestLog <f>.

param(
    [string]$Root = "",
    [string]$PortFile = "",
    [string]$RequestLog = ""
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Root) -or [string]::IsNullOrWhiteSpace($PortFile)) {
    Write-Error "se necesitan -Root y -PortFile"
    exit 2
}
$rootFull = [IO.Path]::GetFullPath($Root)

# Puerto efimero real (HttpListener exige prefijo explicito).
$probe = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
$probe.Start()
$port = $probe.Server.LocalEndPoint.Port
$probe.Stop()

$listener = New-Object Net.HttpListener
$listener.Prefixes.Add("http://127.0.0.1:$port/")
$listener.Start()
Set-Content -Path $PortFile -Value "$port" -NoNewline
try {
    while ($listener.IsListening) {
        $ctx = $listener.GetContext()
        try {
            $rel = $ctx.Request.Url.LocalPath.TrimStart("/")
            Add-Content -Path $RequestLog -Value "$($ctx.Request.HttpMethod) /$rel"
            $full = [IO.Path]::GetFullPath((Join-Path $rootFull ($rel.Replace("/", [IO.Path]::DirectorySeparatorChar))))
            if ($full.StartsWith($rootFull, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path $full -PathType Leaf)) {
                $bytes = [IO.File]::ReadAllBytes($full)
                $ctx.Response.ContentType = "application/octet-stream"
                $ctx.Response.ContentLength64 = $bytes.Length
                $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
            } else {
                $ctx.Response.StatusCode = 404
            }
        } catch {
            $ctx.Response.StatusCode = 500
        } finally {
            $ctx.Response.Close()
        }
    }
} finally {
    $listener.Stop()
}
