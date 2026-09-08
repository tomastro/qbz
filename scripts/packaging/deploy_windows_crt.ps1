# Common app-local payload for the per-user MSI and portable ZIP. No installer,
# elevation, System32 copies, debug runtime or UCRT/API-set bundling.
param([Parameter(Mandatory=$true)][string]$Dist)
$ErrorActionPreference = 'Stop'
$distRoot = (Resolve-Path -LiteralPath $Dist).Path
if (-not $env:VCToolsRedistDir -or -not $env:VCToolsVersion) {
    throw 'Run from the configured MSVC build environment (VCToolsRedistDir/VCToolsVersion required)'
}
$redist = (Resolve-Path -LiteralPath $env:VCToolsRedistDir).Path
$roots = @(Get-ChildItem -LiteralPath (Join-Path $redist 'x64') -Directory |
    Where-Object { $_.Name -match '^Microsoft\.VC\d+\.CRT$' })
if ($roots.Count -ne 1) { throw "Expected one official x64 CRT directory in $redist; found $($roots.Count)" }
$source = $roots[0]
$files = @(Get-ChildItem -LiteralPath $source.FullName -Filter '*.dll' -File)
if ($files.Count -eq 0) { throw 'Official CRT directory contains no DLLs' }
$entries = @()
foreach ($file in $files) {
    if ($file.Name -notmatch '^(msvc[pr]|vcruntime|vccorlib|concrt)\d.*\.dll$' -or $file.Name -match 'd\.dll$') {
        throw "Unexpected DLL in release CRT directory: $($file.Name)"
    }
    $info = $file.VersionInfo
    $fileVersion = "$($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart).$($info.FilePrivatePart)"
    if ([version]$fileVersion -lt [version]$env:VCToolsVersion) { throw "CRT $fileVersion predates toolset $env:VCToolsVersion" }
    Copy-Item -LiteralPath $file.FullName -Destination $distRoot -Force
    $entries += [ordered]@{ name=$file.Name; source=$file.FullName; file_version=$fileVersion;
        sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant();
        copyright=$info.LegalCopyright }
}
$licenses = Join-Path $distRoot 'licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
[ordered]@{ architecture='x64'; toolset_version=$env:VCToolsVersion;
    source=$source.FullName; files=$entries } | ConvertTo-Json -Depth 5 |
    Set-Content -LiteralPath (Join-Path $licenses 'msvc-runtime.json') -Encoding utf8
@'
Microsoft Visual C++ Runtime (x64, application-local deployment)

These runtime DLLs come from the official Visual Studio redistributable
folder. Original version and copyright resources are retained in each DLL;
msvc-runtime.json records their origin, versions, hashes and copyright.
QBZ updates these copies when updating its MSVC runtime dependency.

Microsoft redistribution terms and list:
https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution
Deployment documentation:
https://learn.microsoft.com/en-us/cpp/windows/determining-which-dlls-to-redistribute
'@ | Set-Content -LiteralPath (Join-Path $licenses 'MSVC-RUNTIME-NOTICE.txt') -Encoding utf8
# Retain any notices accompanying the selected official CRT directory.
foreach ($folder in @($source.FullName, $redist)) {
    foreach ($notice in @(Get-ChildItem -LiteralPath $folder -File | Where-Object { $_.Name -match '(?i)(license|notice|redist).*\.txt$' })) {
        Copy-Item -LiteralPath $notice.FullName -Destination (Join-Path $licenses "MSVC-$($notice.Name)") -Force
    }
}
python scripts/packaging/windows_crt.py $distRoot --report windows-crt-imports.json
if ($LASTEXITCODE -ne 0) { throw 'MSVC app-local import audit failed' }
