# Turns a freshly installed Windows guest into a runner for this repository.
#
# Runs once, at the first sign-in after an unattended install. Everything it
# installs is pinned by the caller through the environment, so rebuilding the
# guest a year from now produces the same toolchain rather than whatever is
# current then.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Get-Verified {
    param([string]$Url, [string]$Sha256, [string]$Path)

    Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing
    if ($Sha256) {
        $actual = (Get-FileHash -Algorithm SHA256 -Path $Path).Hash
        if ($actual -ne $Sha256.ToUpper()) {
            throw "checksum mismatch for $Url : expected $Sha256, got $actual"
        }
        return
    }

    # Some vendors publish only a bootstrapper, replaced in place whenever the
    # product moves, so no checksum can be pinned against it. Its signature can:
    # an unsigned or foreign-signed download is refused just as loudly.
    $signature = Get-AuthenticodeSignature -FilePath $Path
    if ($signature.Status -ne 'Valid') {
        throw "$Url is not validly signed: $($signature.Status)"
    }
    if ($signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
        throw "$Url is signed by $($signature.SignerCertificate.Subject), not Microsoft"
    }
}

$settings = Get-Content 'E:\guest.json' -Raw | ConvertFrom-Json
$root = 'C:\kithara-ci'
New-Item -ItemType Directory -Force -Path $root, "$root\downloads" | Out-Null

# The Visual Studio build tools carry the MSVC linker and the Windows SDK,
# without which no Rust target on this platform links at all.
Write-Host '==> Installing the MSVC build tools'
Get-Verified -Url $settings.build_tools_url `
             -Sha256 $settings.build_tools_sha256 `
             -Path "$root\downloads\vs_buildtools.exe"
$arguments = @(
    '--quiet', '--wait', '--norestart', '--nocache',
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.26100',
    '--includeRecommended'
)
$install = Start-Process -FilePath "$root\downloads\vs_buildtools.exe" `
                         -ArgumentList $arguments -Wait -PassThru
# 3010 is "installed, needs a restart", which the guest is about to do anyway.
if ($install.ExitCode -notin 0, 3010) {
    throw "the build tools installer exited with $($install.ExitCode)"
}

Write-Host '==> Installing the Rust toolchain'
Get-Verified -Url $settings.rustup_url `
             -Sha256 $settings.rustup_sha256 `
             -Path "$root\downloads\rustup-init.exe"
& "$root\downloads\rustup-init.exe" `
    -y --no-modify-path --profile minimal `
    --default-toolchain $settings.stable_toolchain
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
[Environment]::SetEnvironmentVariable(
    'PATH',
    "$env:USERPROFILE\.cargo\bin;" + [Environment]::GetEnvironmentVariable('PATH', 'Machine'),
    'Machine')

foreach ($tool in $settings.cargo_tools.PSObject.Properties) {
    Write-Host "==> Installing $($tool.Name) $($tool.Value)"
    cargo install --locked --version $tool.Value $tool.Name
    if ($LASTEXITCODE -ne 0) { throw "cargo install $($tool.Name) failed" }
}

Write-Host '==> Installing the GitHub Actions runner'
New-Item -ItemType Directory -Force -Path "$root\runner" | Out-Null
Get-Verified -Url $settings.runner_url `
             -Sha256 $settings.runner_sha256 `
             -Path "$root\downloads\runner.zip"
Expand-Archive -Path "$root\downloads\runner.zip" -DestinationPath "$root\runner" -Force

# The host mints a fresh just-in-time configuration for every job and drops it
# on the shared volume; the guest never holds a token of its own.
$service = @'
$config = Get-Content E:\jitconfig -Raw
Set-Location C:\kithara-ci\runner
.\run.cmd --jitconfig $config.Trim()
'@
Set-Content -Path "$root\runner\start.ps1" -Value $service -Encoding UTF8

Remove-Item -Recurse -Force "$root\downloads"
Write-Host '==> Guest provisioned'
