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

# The evaluation licence runs ninety days from the image's own release, not
# from installation, and Microsoft leaves an image published far longer than
# that. An expired Windows shuts itself down every hour, which ends a test run
# mid-suite; rearming restarts the period. It is allowed a handful of times,
# which outlives any guest this rebuilds.
$rearm = Start-Process -FilePath 'cscript.exe' `
                       -ArgumentList '//nologo', "$env:SystemRoot\System32\slmgr.vbs", '/rearm' `
                       -Wait -PassThru -NoNewWindow
if ($rearm.ExitCode -ne 0) {
    Write-Warning "could not rearm the evaluation licence (exit $($rearm.ExitCode))"
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

# What the guest does on every sign-in from here on. It registers once, with
# credentials the host leaves on the answer volume, and then serves jobs until
# it is restarted. The registration outlives a restart, so the enrolment branch
# is taken exactly once per installed guest; a guest that boots before the host
# has left it anything says so and stops rather than looking busy.
$runner = @'
Set-Location C:\kithara-ci\runner
if (-not (Test-Path '.runner')) {
    if (-not (Test-Path 'E:\enrolment.json')) {
        Write-Host 'no enrolment on E:; nothing to register with'
        exit 1
    }
    $enrolment = Get-Content 'E:\enrolment.json' -Raw | ConvertFrom-Json
    .\config.cmd --unattended --replace --work _work `
                 --url $enrolment.url --token $enrolment.token `
                 --name $enrolment.name --labels $enrolment.labels
    if ($LASTEXITCODE -ne 0) { throw "runner enrolment failed with $LASTEXITCODE" }
}
.\run.cmd
'@
Set-Content -Path "$root\runner\start.ps1" -Value $runner -Encoding UTF8

# Windows runs whatever is in this folder at sign-in, which needs no scheduled
# task and no password to register one with.
$startup = [Environment]::GetFolderPath('Startup')
Set-Content -Path "$startup\kithara-ci-runner.cmd" `
            -Value "powershell -NoProfile -ExecutionPolicy Bypass -File $root\runner\start.ps1" `
            -Encoding ASCII

Remove-Item -Recurse -Force "$root\downloads"
Write-Host '==> Guest provisioned'

# The sign-in that ran this one was granted by the answer file; every later one
# is the automatic sign-in, which only takes effect on a restart.
Restart-Computer -Force
