# Creates two small NTFS volumes on virtual disks, V: and W:, for the live snapshot tests.
# W: is also mounted on the folder V:\mnt\w, to test volumes mounted on folders.
# Snapshotting these takes seconds, where the runner's system drive would take much longer.
# Each disk is expandable, so it only uses the space that's written to it.

$ErrorActionPreference = 'Stop'

foreach ($letter in 'V', 'W') {
    $vhd = Join-Path $env:RUNNER_TEMP "test-volume-$letter.vhdx"
    $script = Join-Path $env:RUNNER_TEMP "test-volume-$letter.txt"
    @(
        "create vdisk file=`"$vhd`" maximum=1024 type=expandable",
        "select vdisk file=`"$vhd`"",
        'attach vdisk',
        'create partition primary',
        "format fs=ntfs quick label=Test$letter",
        "assign letter=$letter"
    ) | Set-Content -Encoding ascii $script
    diskpart /s $script
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$folder = 'V:\mnt\w'
New-Item -ItemType Directory -Path $folder | Out-Null
$guidPath = (Get-Volume -DriveLetter W).Path
mountvol $folder $guidPath
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Get-Volume -DriveLetter V, W | Format-Table DriveLetter, FileSystem, Size, SizeRemaining, Path
mountvol | Select-String -Pattern 'V:\\mnt\\w' -Context 1, 0
