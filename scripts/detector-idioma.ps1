# Detector de identificadores en espanol para codigo de primera parte.
# Escanea caracter a caracter, con maquina de estados, para no confundir
# comentarios ni literales con codigo. Devuelve codigo de salida 1 si hay
# identificadores en espanol.

param([string]$Root = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$repo = $Root

# Palabras castellanas que se consideran marca de identificador en espanol.
# Cualquier palabra que tambien sea ingles valido queda fuera a proposito.
$ES = @(
  'ancestro','anunciado','archivo','archivos','atribuciones','cambiado','candidatos',
  'caso','causa','clave','codigo','cobertura','contenido','contenidos','convertir',
  'crear','curado','dentro','descargalos','desarrollo','desinstalacion','despues',
  'destino','directorio','directorios','durante','envenenado','escribir','estado',
  'existe','fallar','fallido','fallo','fichero','ficheros','fuente','habla','hasta',
  'hecho','incluido','incluidos','inexistente','instalado','leido','leidos',
  'limpiado','limpiar','linea','lineas','marca','marcado','marcar','mensaje',
  'metodo','modelo','modelos','modo','nunca','numero','numeros','oferta','orden',
  'padre','pregunta','preguntados','previo','primera','prohibido','programa',
  'propio','propios','publicada','publicado','razon','raiz','raices','recibo',
  'resultado','seccion','seleccion','sembrar','separador','simulacion','sobre',
  'tamano','temporal','temporales','tenemos','texto','tiene','tienen','tipo',
  'tipos','unicos','unico','vacios','vacio','valor','valores','vecina','verbo',
  'viajan','vienen','vistas','vivo','residente','diario','registro','entradas',
  'canal','enlazado','falta','faltan','esperado','esperada','duplicado','duplicada',
  'interrumpir','integracion','recuperacion','instalacion','sincronizar','pendiente',
  'pendientes','proceso','procesos','sesion','transaccion','verificar','comprobar',
  'ejecutar','leer','nuevo','nueva','anterior','siguiente','copia','copias','mover',
  'abrir','cerrar','listo','activo','inactivo','principal','secundario','exito',
  'borrar','nombre','nombres','ruta','rutas','usuario','usuarios','operacion',
  'huerfano','huerfanos','resumen','correcto','siguiente','resto','madre',
  'anadir','solo'
)
# `todo` se deja fuera a proposito: en este codigo es el marcador ingles del
# CHANGELOG, no la palabra castellana. `with_todo` en xtask lo demuestra.

# La lista esta escrita sin acentos, y el comparador es OrdinalIgnoreCase, que no
# los iguala: sin normalizar, `codigo` con tilde pasaria el filtro. Se quitan los
# diacriticos antes de comparar, no al construir la lista, que ya es ASCII.
function Remove-Diacritics([string]$s) {
  $d = $s.Normalize([System.Text.NormalizationForm]::FormD)
  $sb = [System.Text.StringBuilder]::new($d.Length)
  foreach ($ch in $d.ToCharArray()) {
    $cat = [System.Globalization.CharUnicodeInfo]::GetUnicodeCategory($ch)
    if ($cat -ne [System.Globalization.UnicodeCategory]::NonSpacingMark) { [void]$sb.Append($ch) }
  }
  $sb.ToString().Normalize([System.Text.NormalizationForm]::FormC)
}
$set = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$ES | ForEach-Object { [void]$set.Add($_) }

# Devuelve solo el codigo: comentarios, literales y cadenas fuera de cuenta.
function Get-Code([string]$text) {
  $sb = [System.Text.StringBuilder]::new($text.Length)
  $i = 0; $n = $text.Length
  while ($i -lt $n) {
    $c = $text[$i]
    if ($c -eq '/' -and $i + 1 -lt $n -and $text[$i+1] -eq '/') {
      while ($i -lt $n -and $text[$i] -ne "`n") { $i++ }
      continue
    }
    if ($c -eq '/' -and $i + 1 -lt $n -and $text[$i+1] -eq '*') {
      $depth = 1; $i += 2
      while ($i -lt $n -and $depth -gt 0) {
        if ($text[$i] -eq '/' -and $i + 1 -lt $n -and $text[$i+1] -eq '*') { $depth++; $i += 2; continue }
        if ($text[$i] -eq '*' -and $i + 1 -lt $n -and $text[$i+1] -eq '/') { $depth--; $i += 2; continue }
        if ($text[$i] -eq "`n") { [void]$sb.Append("`n") }
        $i++
      }
      continue
    }
    if ($c -eq 'r' -and $i + 1 -lt $n -and ($text[$i+1] -eq '"' -or $text[$i+1] -eq '#')) {
      $j = $i + 1; $hashes = 0
      while ($j -lt $n -and $text[$j] -eq '#') { $hashes++; $j++ }
      if ($j -lt $n -and $text[$j] -eq '"') {
        $j++; $term = '"' + ('#' * $hashes)
        $end = $text.IndexOf($term, $j)
        if ($end -lt 0) { $i = $n } else { $i = $end + $term.Length }
        [void]$sb.Append(' '); continue
      }
    }
    if ($c -eq '"') {
      $i++; while ($i -lt $n) {
        if ($text[$i] -eq '\') { $i += 2; continue }
        if ($text[$i] -eq '"') { $i++; break }
        if ($text[$i] -eq "`n") { [void]$sb.Append("`n") }
        $i++
      }
      [void]$sb.Append(' '); continue
    }
    if ($c -eq "'") {
      # literal de caracter o lifetime: solo se consume si cierra en 4 caracteres
      if ($i + 2 -lt $n -and $text[$i+2] -eq "'") { $i += 3; [void]$sb.Append(' '); continue }
      [void]$sb.Append($c); $i++; continue
    }
    # Apende directo. Se probó optimizar con IndexOfAny por tramo y fue peor:
    # 25 s frente a 10 s en el mismo árbol, y rompió el stripping de comentarios
    # en los ficheros con raw strings. Se queda la version correcta y simple.
    [void]$sb.Append($c); $i++
  }
  $sb.ToString()
}

# Parte un identificador en sus segmentos, tanto por guion bajo como por
# frontera camelCase: sin esto, resultadoFinal y ruta_destino son invisibles.
function Get-Segments([string]$id) {
  $out = [System.Collections.Generic.List[string]]::new()
  foreach ($chunk in $id.Split('_')) {
    if ([string]::IsNullOrEmpty($chunk)) { continue }
    foreach ($p in [regex]::Split($chunk, '(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])')) {
      if ($p) { $out.Add((Remove-Diacritics $p)) }
    }
  }
  return $out
}

# Unicode y no ASCII: Rust admite identificadores no ASCII, y una letra fuera de
# la clase ASCII rompia la tokenizacion. `anadir` se leia como `adir`, que
# ademas caia por el gate de longitud y era invisible.
$id = [regex]'[_\p{L}][\p{L}\p{N}_]*'
$hits = @()
# Raices de codigo de primera parte. scripts/ entra por si algun dia aloja Rust;
# este fichero .ps1 no lo cubre, porque el lexificador es el de Rust.
$roots = @("$repo\src", "$repo\crates", "$repo\tests", "$repo\scripts")
$files = $roots |
  Where-Object { Test-Path $_ } |
  ForEach-Object { Get-ChildItem $_ -Recurse -File -Filter *.rs } |
  Where-Object { $_.FullName -notmatch '\\vendor\\' }
foreach ($f in $files) {
  $code = Get-Code ([System.IO.File]::ReadAllText($f.FullName))
  foreach ($m in $id.Matches($code)) {
    # 4 y no 5: `solo` y `caso` son tan castellanas como `linea`.
    $segs = @(Get-Segments $m.Value | Where-Object { $_.Length -ge 4 -and $set.Contains($_) })
    if ($segs.Count) {
      $hits += [pscustomobject]@{
        file = $f.FullName.Substring($repo.Length + 1)
        id   = $m.Value
        word = ($segs -join ',')
      }
    }
  }
}
if ($hits.Count) {
  $hits | Sort-Object file, id | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
  "IDENTIFICADORES EN ESPANOL: $($hits.Count) en $(($hits | Select-Object -ExpandProperty file -Unique).Count) ficheros"
  exit 1
}
"0 identificadores en espanol en $($files.Count) ficheros de primera parte"
exit 0
