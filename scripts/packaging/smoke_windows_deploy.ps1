# Smoke the actual deployed product. qoffscreen.dll is neither injected nor
# resolved from the SDK. A development machine still does not prove clean OS.
param(
    [Parameter(Mandatory=$true)][string]$Directory,
    [Parameter(Mandatory=$true)][string]$LogPrefix,
    [int]$Seconds = 75
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Directory).Path
$prefix = [IO.Path]::GetFullPath($LogPrefix)
$manifest = Get-Content -LiteralPath (Join-Path $root 'licenses/msvc-runtime.json') -Raw | ConvertFrom-Json
# QML/platform/image plugins do not all have a Qt6-prefixed filename.
$qtModules = @(Get-ChildItem -LiteralPath $root -Recurse -Filter '*.dll' -File |
    Where-Object { $_.DirectoryName -ne $root -or $_.Name -match '^Qt6' } | ForEach-Object { $_.Name })
$profile = Join-Path ([IO.Path]::GetTempPath()) ('qbz-deploy-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $profile | Out-Null
$names = @('PATH','APPDATA','LOCALAPPDATA','QT_PLUGIN_PATH','QML_IMPORT_PATH','QML2_IMPORT_PATH',
    'QT_ROOT_DIR','QTDIR','QT_QPA_PLATFORM_PLUGIN_PATH','QT_QPA_PLATFORM','QSG_RHI_BACKEND',
    'QT_QUICK_BACKEND','QBZ_RENDERER','RUST_LOG')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$app = $null
try {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $null, 'Process') }
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $env:APPDATA = Join-Path $profile 'roaming'
    $env:LOCALAPPDATA = Join-Path $profile 'local'
    New-Item -ItemType Directory -Path $env:APPDATA,$env:LOCALAPPDATA | Out-Null
    $env:QT_QPA_PLATFORM = 'windows'
    $env:RUST_LOG = 'info'
    $app = Start-Process -FilePath (Join-Path $root 'qbz.exe') -WorkingDirectory $root `
        -RedirectStandardOutput "$prefix.out" -RedirectStandardError "$prefix.err" -PassThru
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($clock.Elapsed.TotalSeconds -lt $Seconds) {
        if ($app.HasExited) { throw "Product exited before smoke deadline: $($app.ExitCode)" }
        Start-Sleep -Milliseconds 250
        $app.Refresh()
    }
    if ($app.MainWindowHandle -eq 0) { throw 'Product did not create a visible native window' }
    $modules = @($app.Modules | ForEach-Object { [ordered]@{ name=$_.ModuleName; path=$_.FileName } })
    $modules | ConvertTo-Json | Set-Content -LiteralPath "$prefix.modules.json"
    foreach ($entry in $modules) {
        $isCrt = @($manifest.files | Where-Object { $_.name -ieq $entry.name }).Count -gt 0
        if ($isCrt -or $entry.name -in $qtModules) {
            if (-not $entry.path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) {
                throw "Product resolved $($entry.name) outside deployment: $($entry.path)"
            }
        }
    }
    foreach ($required in @('qwindows.dll','Qt6Core.dll','vcruntime140.dll')) {
        if (-not ($modules | Where-Object { $_.name -ieq $required })) { throw "Expected loaded module missing: $required" }
    }
}
finally {
    if ($null -ne $app -and -not $app.HasExited) {
        Stop-Process -Id $app.Id -Force
        $app.WaitForExit(5000) | Out-Null
    }
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    Remove-Item -LiteralPath $profile -Recurse -Force
    if ((Test-Path "$prefix.out") -and (Test-Path "$prefix.err")) {
        Get-Content "$prefix.out","$prefix.err" | Set-Content "$prefix.log"
    }
}
$lines = @(Get-Content -LiteralPath "$prefix.log")
$pattern = 'is not a type|unavailable|ReferenceError|TypeError|Cannot read|Unable to assign|Cannot open|no such method|non-existent property|failed to load component|is not installed|could not be found|0xc0000135|STATUS_DLL_NOT_FOUND|This application failed to start'
$errors = @($lines | Where-Object { $_ -notmatch 'propertyCache' -and $_ -match $pattern })
if ($errors.Count -gt 0) { $errors | Select-Object -First 20 | Write-Host; throw 'QML/loader complaints in deployed smoke' }
if (-not ($lines -match 'QbzCore initialized')) { throw 'Product never reached core initialization' }
Write-Host "Deployed GUI smoke OK: $root, survived ${Seconds}s, app-local Qt/CRT modules"
