param(
    [Parameter(Mandatory = $true)]
    [string] $PlaygroundExecutable,
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,
    [int] $Number = 32,
    [int] $ClientWidth = 1920,
    [int] $ClientHeight = 1080
)

$ErrorActionPreference = 'Stop'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$isAdministrator = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdministrator) {
    $playgroundPath = [IO.Path]::GetFullPath((Join-Path $PWD $PlaygroundExecutable))
    $outputPath = [IO.Path]::GetFullPath((Join-Path $PWD $OutputDirectory))
    $relaunchArguments = @(
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-File', ('"{0}"' -f $PSCommandPath),
        '-PlaygroundExecutable', ('"{0}"' -f $playgroundPath),
        '-OutputDirectory', ('"{0}"' -f $outputPath),
        '-Number', $Number,
        '-ClientWidth', $ClientWidth,
        '-ClientHeight', $ClientHeight
    ) -join ' '
    $elevated = Start-Process powershell.exe -Verb RunAs -ArgumentList $relaunchArguments -Wait -PassThru
    exit $elevated.ExitCode
}

$fixtureDirectory = Join-Path $OutputDirectory 'fixture'
$scanDirectory = Join-Path $OutputDirectory 'scan'
New-Item -ItemType Directory -Force -Path $fixtureDirectory, $scanDirectory | Out-Null

$fixtureLog = Join-Path $fixtureDirectory 'process.log'
$env:YAS_E2E_ERROR_FILE = Join-Path $fixtureDirectory 'error.txt'
& $PlaygroundExecutable --prepare-artifact-page $ClientWidth $ClientHeight $fixtureDirectory *> $fixtureLog
if ($LASTEXITCODE -ne 0) {
    Write-Error "Genshin artifact-page fixture failed; see $fixtureLog"
    exit $LASTEXITCODE
}

# Let Genshin fully regain foreground input after the fixture process exits.
Start-Sleep -Milliseconds 1500

$scanLog = Join-Path $scanDirectory 'process.log'
$env:YAS_E2E_ERROR_FILE = Join-Path $scanDirectory 'error.txt'
& $PlaygroundExecutable --traverse-probe $Number $scanDirectory *> $scanLog
if ($LASTEXITCODE -ne 0) {
    Write-Error "Genshin artifact traversal failed; see $scanLog"
    exit $LASTEXITCODE
}

Write-Output "fixture=$fixtureDirectory"
Write-Output "scan=$scanDirectory"
