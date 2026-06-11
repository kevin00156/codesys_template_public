#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('restart-codesyscontrol', 'restart-plc_bridge', 'deploy', 'shm-reset', 'setup-sudoers')]
    [string] $Mode,

    [string] $TargetHost = '',
    [string] $TargetUser = '',
    [string] $WslDistro  = '',
    [int]    $TargetPort = 22,
    [int]    $TimeoutMs  = 800
)

$ErrorActionPreference = 'Stop'

function Get-EnvValue {
    param([string]$Key, [string]$Default)
    $envFile = Join-Path $PSScriptRoot '..\.env'
    if (Test-Path $envFile) {
        $line = Get-Content $envFile | Where-Object { $_ -match "^\s*$Key\s*=\s*(.+?)\s*$" } | Select-Object -First 1
        if ($line) { return $matches[1] }
    }
    return $Default
}

if (-not $TargetHost) {
    $TargetHost = Get-EnvValue -Key 'PLC_HOST' -Default '192.168.1.10'
}
if (-not $TargetUser) {
    $TargetUser = Get-EnvValue -Key 'PLC_USER' -Default 'plc'
}
if (-not $WslDistro) {
    $WslDistro = Get-EnvValue -Key 'WSL_DISTRO' -Default 'Ubuntu-22.04'
}

$commands = @{
    'restart-codesyscontrol' = @{
        Remote = "ssh ${TargetUser}@${TargetHost} 'sudo -n systemctl restart codesyscontrol.service && echo OK'"
        Wsl    = "wsl -d ${WslDistro} -- bash -lc 'sudo -n systemctl restart codesyscontrol.service && echo OK'"
    }
    'restart-plc_bridge' = @{
        Remote = "ssh ${TargetUser}@${TargetHost} 'sudo -n systemctl restart plc_bridge && echo OK'"
        Wsl    = "wsl -d ${WslDistro} -- bash -lc 'sudo -n systemctl restart plc_bridge && echo OK'"
    }
    'deploy' = @{
        Remote = 'make deploy'
        Wsl    = 'make wsl-deploy'
    }
    'shm-reset' = @{
        Remote = 'make shm-reset'
        Wsl    = 'make wsl-shm-reset'
    }
    'setup-sudoers' = @{
        Remote = 'make setup-sudoers'
        Wsl    = 'make wsl-setup-sudoers'
    }
}

function Test-RemoteReachable {
    param([string]$ProbeHost, [int]$Port, [int]$TimeoutMs)
    $tcp = New-Object System.Net.Sockets.TcpClient
    try {
        $task = $tcp.ConnectAsync($ProbeHost, $Port)
        if ($task.Wait($TimeoutMs)) { return $tcp.Connected }
        return $false
    } catch {
        return $false
    } finally {
        $tcp.Close()
    }
}

$cmd = $commands[$Mode]

if (Test-RemoteReachable -ProbeHost $TargetHost -Port $TargetPort -TimeoutMs $TimeoutMs) {
    Write-Host "[remote] ${TargetHost}:${TargetPort} reachable -> industrial PC" -ForegroundColor Cyan
    Write-Host "  > $($cmd.Remote)" -ForegroundColor DarkGray
    Invoke-Expression $cmd.Remote
} else {
    Write-Host "[wsl-fallback] $TargetHost unreachable -> WSL $WslDistro" -ForegroundColor Yellow
    Write-Host "  > $($cmd.Wsl)" -ForegroundColor DarkGray
    Invoke-Expression $cmd.Wsl
}

exit $LASTEXITCODE
