# Run every agentskillpack demo end to end. Exits 0 only if all demos pass.
#
# Usage:  pwsh demos/run_all.ps1   (or: powershell -File demos/run_all.ps1)
$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here
Set-Location $root

Write-Host "== building agentskillpack =="
cargo build --quiet
if ($LASTEXITCODE -ne 0) { throw "build failed" }

$bin = Join-Path $root "target/debug/agentskillpack.exe"
if (-not (Test-Path $bin)) { $bin = Join-Path $root "target/debug/agentskillpack" }

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("asp_demo_" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $work | Out-Null

$script:pass = 0
$script:fail = 0

function Invoke-Demo {
    param([string]$Name, [scriptblock]$Body)
    Write-Host ""
    Write-Host "== demo: $Name =="
    try {
        & $Body
        Write-Host "-- ${Name}: PASS"
        $script:pass++
    } catch {
        Write-Host "-- ${Name}: FAIL ($_)"
        $script:fail++
    }
}

Invoke-Demo "lifecycle (pack->sign->verify->registry->resolve->lock->unpack)" {
    $arc = Join-Path $work "hello.skillpack"
    & $bin pack examples/hello-skill -o $arc --validate; if ($LASTEXITCODE) { throw "pack" }
    & $bin info $arc; if ($LASTEXITCODE) { throw "info" }
    & $bin keygen -o (Join-Path $work "keys") --name author; if ($LASTEXITCODE) { throw "keygen" }
    & $bin sign $arc --key (Join-Path $work "keys/author.key"); if ($LASTEXITCODE) { throw "sign" }
    & $bin verify $arc --pubkey (Join-Path $work "keys/author.pub"); if ($LASTEXITCODE) { throw "verify" }
    $reg = Join-Path $work "registry"
    & $bin registry add $arc --registry $reg; if ($LASTEXITCODE) { throw "registry add" }
    & $bin registry list --registry $reg; if ($LASTEXITCODE) { throw "registry list" }
    & $bin registry resolve hello-skill --req '^1.0' --registry $reg; if ($LASTEXITCODE) { throw "resolve" }
    $rarc = Join-Path $work "research.skillpack"
    & $bin pack examples/research-skill -o $rarc --validate; if ($LASTEXITCODE) { throw "pack research" }
    & $bin registry add $rarc --registry $reg; if ($LASTEXITCODE) { throw "add research" }
    & $bin lock examples/research-skill --registry $reg -o (Join-Path $work "skillpack.lock"); if ($LASTEXITCODE) { throw "lock" }
    Get-Content (Join-Path $work "skillpack.lock")
    & $bin unpack $arc -o (Join-Path $work "restored"); if ($LASTEXITCODE) { throw "unpack" }
    if (-not (Test-Path (Join-Path $work "restored/scripts/greet.py"))) { throw "unpack missing file" }
}

Invoke-Demo "tamper detection" {
    $arc = Join-Path $work "tamper.skillpack"
    & $bin pack examples/hello-skill -o $arc; if ($LASTEXITCODE) { throw "pack" }
    $bytes = [System.IO.File]::ReadAllBytes($arc)
    $bytes[$bytes.Length - 3] = $bytes[$bytes.Length - 3] -bxor 0xFF
    [System.IO.File]::WriteAllBytes($arc, $bytes)
    & $bin verify $arc | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "tampered archive verified (should not)" }
    Write-Host "tamper correctly rejected"
}

Invoke-Demo "wrong-key rejection" {
    $arc = Join-Path $work "wk.skillpack"
    & $bin pack examples/hello-skill -o $arc; if ($LASTEXITCODE) { throw "pack" }
    & $bin keygen -o (Join-Path $work "k1") --name signer; if ($LASTEXITCODE) { throw "keygen1" }
    & $bin keygen -o (Join-Path $work "k2") --name other; if ($LASTEXITCODE) { throw "keygen2" }
    & $bin sign $arc --key (Join-Path $work "k1/signer.key"); if ($LASTEXITCODE) { throw "sign" }
    & $bin verify $arc --pubkey (Join-Path $work "k2/other.pub") | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "wrong key accepted (should not)" }
    Write-Host "wrong key correctly rejected"
}

Invoke-Demo "manifest validation" {
    & $bin manifest validate examples/research-skill; if ($LASTEXITCODE) { throw "valid manifest rejected" }
    $bad = Join-Path $work "bad"
    New-Item -ItemType Directory -Force -Path $bad | Out-Null
    '{"name":"Bad Name","version":"nope"}' | Out-File -FilePath (Join-Path $bad "skill.json") -Encoding ascii
    & $bin manifest validate $bad | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "invalid manifest passed (should not)" }
    Write-Host "invalid manifest correctly rejected"
}

Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "==================================="
Write-Host "demos passed: $($script:pass)  failed: $($script:fail)"
Write-Host "==================================="
if ($script:fail -ne 0) { exit 1 }
exit 0
