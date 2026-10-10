[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ApplicationDirectory,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Za-z0-9.]+$')]
    [string]$IdentityName,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Fa-f0-9 ]{40,59}$')]
    [string]$CertificateThumbprint,

    [string]$Version,

    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$applicationPath = (Resolve-Path $ApplicationDirectory).Path
$applicationExe = Join-Path $applicationPath 'venu.exe'
if (-not (Test-Path -LiteralPath $applicationExe -PathType Leaf)) {
    throw "ApplicationDirectory must contain venu.exe: $applicationPath"
}

$osVersion = [Environment]::OSVersion.Version
if ($osVersion.Major -lt 10 -or ($osVersion.Major -eq 10 -and $osVersion.Build -lt 19041)) {
    throw 'Sparse external-location identity packages require Windows 10 build 19041 or newer.'
}

$makeAppx = Get-Command 'MakeAppx.exe' -ErrorAction SilentlyContinue
$signTool = Get-Command 'SignTool.exe' -ErrorAction SilentlyContinue
if (-not $makeAppx -or -not $signTool) {
    throw 'Install the Windows SDK and add its MakeAppx.exe and SignTool.exe tools to PATH.'
}

$thumbprint = ($CertificateThumbprint -replace '\s', '').ToUpperInvariant()
if ($thumbprint.Length -ne 40) {
    throw 'CertificateThumbprint must contain exactly 40 hexadecimal characters.'
}
$certificate = Get-ChildItem 'Cert:\CurrentUser\My' |
    Where-Object { $_.Thumbprint -eq $thumbprint } |
    Select-Object -First 1
if (-not $certificate -or -not $certificate.HasPrivateKey) {
    throw "No signing certificate with a private key was found in CurrentUser\My for $thumbprint."
}

$publisher = $certificate.Subject
if ([string]::IsNullOrWhiteSpace($publisher)) {
    throw 'The selected signing certificate has no subject to use as the package publisher.'
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $cargoManifest = Get-Content -LiteralPath (Join-Path $repoRoot 'Cargo.toml')
    $versionLine = $cargoManifest | Select-String '^version\s*=\s*"([^"+]+)' | Select-Object -First 1
    if (-not $versionLine) {
        throw 'Could not read the Venu version from Cargo.toml.'
    }
    $Version = $versionLine.Matches[0].Groups[1].Value
}
if ($Version -match '^\d+\.\d+\.\d+$') {
    $Version = "$Version.0"
}
if ($Version -notmatch '^\d{1,5}\.\d{1,5}\.\d{1,5}\.\d{1,5}$') {
    throw 'Version must contain three or four numeric components, such as 0.4.3 or 0.4.3.0.'
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $repoRoot 'dist\venu-identity.msix'
}
$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputDirectory = Split-Path -Parent $outputFullPath
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("venu-identity-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Assets') -Destination $stage -Recurse
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'AppxManifest.xml') -Destination $stage

    $manifestPath = Join-Path $stage 'AppxManifest.xml'
    [xml]$manifest = Get-Content -LiteralPath $manifestPath -Raw
    $identity = $manifest.Package.Identity
    $identity.SetAttribute('Name', $IdentityName)
    $identity.SetAttribute('Publisher', $publisher)
    $identity.SetAttribute('Version', $Version)
    $manifest.Save($manifestPath)

    $unsignedPath = "$stage.msix"
    & $makeAppx.Source pack /o /d $stage /nv /p $unsignedPath
    if ($LASTEXITCODE -ne 0) {
        throw "MakeAppx failed with exit code $LASTEXITCODE."
    }

    & $signTool.Source sign /fd SHA256 /sha1 $thumbprint /s My /v $unsignedPath
    if ($LASTEXITCODE -ne 0) {
        throw "SignTool failed with exit code $LASTEXITCODE. No package was published."
    }
    & $signTool.Source verify /pa /v $unsignedPath
    if ($LASTEXITCODE -ne 0) {
        throw "SignTool could not verify the signed package. No package was published."
    }

    Move-Item -LiteralPath $unsignedPath -Destination $outputFullPath -Force
    Write-Output "Signed identity package: $outputFullPath"
    Write-Output "External application directory: $applicationPath"
    Write-Output 'Registration is a separate, user-initiated step documented in packaging/identity/README.md.'
}
finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath "$stage.msix" -Force -ErrorAction SilentlyContinue
}
