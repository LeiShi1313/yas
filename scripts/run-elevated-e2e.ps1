param(
    [Parameter(Mandatory = $true)]
    [string] $Executable,
    [Parameter(Mandatory = $true)]
    [string] $Command,
    [Parameter(Mandatory = $true)]
    [int] $Number,
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory
)

$logPath = Join-Path $OutputDirectory 'process.log'
& $Executable $Command $Number $OutputDirectory *> $logPath
exit $LASTEXITCODE
