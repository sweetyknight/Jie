# release 构建，把 jie.exe 打成 dist\Jie-<版本>-portable.zip；解压出来是 Jie-<版本>\jie.exe
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '../..')
cargo build --release
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$ver = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$stage = "dist\Jie-$ver"
$zip = "dist\Jie-$ver-portable.zip"
foreach ($p in $stage, $zip) { if (Test-Path $p) { Remove-Item $p -Recurse -Force } }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\jie.exe $stage
# Windows PowerShell 5.1 默认用反斜杠写 zip 里的路径（Compress-Archive 也是），zip 规范要正斜杠；关掉这个兼容开关
[AppContext]::SetSwitch('Switch.System.IO.Compression.ZipFile.UseBackslash', $false)
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory((Resolve-Path $stage).Path, (Join-Path $PWD $zip), 'Optimal', $true)
Remove-Item $stage -Recurse -Force
"$zip  $([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB"
