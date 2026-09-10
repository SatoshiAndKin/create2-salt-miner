param([Parameter(Mandatory=$true)][string]$Bundle)
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne 'Win32NT') { throw 'Train on native Windows with an OpenCL driver.' }
$Bundle = (Resolve-Path $Bundle).Path
$TrainingDirectory = Join-Path ([IO.Path]::GetTempPath()) ('salty-pgo-' + [guid]::NewGuid())
New-Item -ItemType Directory $TrainingDirectory | Out-Null
Expand-Archive -LiteralPath $Bundle -DestinationPath $TrainingDirectory
$Metadata = Get-Content (Join-Path $TrainingDirectory 'metadata.json') -Raw | ConvertFrom-Json
if ($Metadata.target -ne 'x86_64-pc-windows-gnu') { throw 'Wrong PGO target.' }
$Executable = Join-Path $TrainingDirectory 'salty.exe'
$ExecutableHash = (Get-FileHash $Executable -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ExecutableHash -ne $Metadata.executable_sha256) { throw 'Executable checksum mismatch.' }
$RawDirectory = Join-Path $TrainingDirectory 'raw'
New-Item -ItemType Directory $RawDirectory | Out-Null
$OldProfileFile = $env:LLVM_PROFILE_FILE
Push-Location $TrainingDirectory
try {
    $env:LLVM_PROFILE_FILE = Join-Path $RawDirectory '%m-%p.profraw'
    & $Executable list
    if ($LASTEXITCODE -ne 0) { throw 'No usable OpenCL device.' }
    $Inputs = $Metadata.training
    & $Executable mine --factory $Inputs.factory --caller $Inputs.caller --codehash $Inputs.codehash --worksize $Inputs.worksize --zeros $Inputs.zeros --min-runtime-secs $Inputs.min_runtime_secs --abi
    $TrainingExitCode = $LASTEXITCODE
    if ($TrainingExitCode -ne 0) { throw "Mining training failed: $TrainingExitCode" }
    if (!(Get-ChildItem $RawDirectory -Filter '*.profraw' | Where-Object Length -gt 0)) { throw 'No raw profiles were collected.' }
    @{
        platform = 'Windows'
        os = [Environment]::OSVersion.VersionString
        architecture = $env:PROCESSOR_ARCHITECTURE
        executable_sha256 = $ExecutableHash
        exit_code = $TrainingExitCode
        completed_utc = [DateTime]::UtcNow.ToString('o')
    } | ConvertTo-Json | Set-Content (Join-Path $TrainingDirectory 'training.json') -Encoding UTF8
    $Results = $Bundle + '.results.zip'
    if (Test-Path $Results) { throw "Results already exist: $Results" }
    Compress-Archive -Path 'metadata.json','training.json','raw' -DestinationPath $Results
    Write-Output $Results
} finally {
    Pop-Location
    $env:LLVM_PROFILE_FILE = $OldProfileFile
}
