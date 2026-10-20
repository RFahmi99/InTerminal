# VCE shell integration. Runs once inside the user's PowerShell session (after their
# profile) and reports prompt state to the VCE app over a private named pipe, so the
# app never has to scrape the terminal output stream. It must work on Windows
# PowerShell 5.1 and PowerShell 7, and it must never break the user's prompt: every
# failure degrades to a plain shell with a single warning.

if (-not $env:VCE_PIPE) { return }

try {
    $__vcePipeName = $env:VCE_PIPE -replace '^\\\\\.\\pipe\\', ''
    Remove-Item Env:\VCE_PIPE
    $__vcePipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $__vcePipeName, [System.IO.Pipes.PipeDirection]::Out)
    $__vcePipe.Connect(5000)
    $global:__VceWriter = New-Object System.IO.StreamWriter($__vcePipe, (New-Object System.Text.UTF8Encoding($false)))
    $global:__VceWriter.AutoFlush = $true
} catch {
    Write-Warning "VCE could not connect to the app ($($_.Exception.Message)). The terminal works normally, but the file tree and error help will not follow this session."
    return
} finally {
    Remove-Variable -Name __vcePipeName, __vcePipe -ErrorAction SilentlyContinue
}

$global:__VceHasPSReadLine = [bool](Get-Command Get-PSReadLineOption -ErrorAction SilentlyContinue)
$global:__VceOriginalPrompt = $function:prompt
$global:__VceLastHistoryId = (Get-History -Count 1).Id
$global:__VceLastError = if ($global:Error.Count) { $global:Error[0] } else { $null }

function global:__VceSend($message) {
    if (-not $global:__VceWriter) { return }
    try {
        $global:__VceWriter.WriteLine(($message | ConvertTo-Json -Compress -Depth 4))
    } catch {
        $global:__VceWriter = $null
        Write-Warning "VCE lost its connection to the app ($($_.Exception.Message)). The terminal keeps working without the file tree and error help."
    }
}

__VceSend @{
    type      = 'hello'
    psVersion = $PSVersionTable.PSVersion.ToString()
    edition   = "$($PSVersionTable.PSEdition)"
    osVersion = [Environment]::OSVersion.VersionString
    psReadLine = $global:__VceHasPSReadLine
}

function global:prompt {
    # $? and $LASTEXITCODE must be read before any other statement overwrites them.
    $vceSuccess = $global:?
    $vceExitCode = $global:LASTEXITCODE

    $last = Get-History -Count 1
    $ran = [bool]$last -and $last.Id -ne $global:__VceLastHistoryId
    if ($last) { $global:__VceLastHistoryId = $last.Id }

    $errorInfo = $null
    $newestError = if ($global:Error.Count) { $global:Error[0] } else { $null }
    if ($ran -and -not $vceSuccess -and $newestError -and -not [object]::ReferenceEquals($newestError, $global:__VceLastError)) {
        $message = "$($newestError.Exception.Message)"
        if ($message.Length -gt 2000) { $message = $message.Substring(0, 2000) }
        $errorInfo = @{
            message       = $message
            errorId       = "$($newestError.FullyQualifiedErrorId)"
            category      = "$($newestError.CategoryInfo.Category)"
            target        = "$($newestError.TargetObject)"
            exceptionType = $newestError.Exception.GetType().FullName
        }
    }
    $global:__VceLastError = $newestError

    $location = $executionContext.SessionState.Path.CurrentLocation
    $editMode = $null
    if ($global:__VceHasPSReadLine) {
        try { $editMode = "$((Get-PSReadLineOption).EditMode)" } catch { $editMode = $null }
    }

    __VceSend @{
        type        = 'prompt'
        cwd         = $location.ProviderPath
        provider    = $location.Provider.Name
        ran         = $ran
        commandLine = if ($ran) { $last.CommandLine } else { $null }
        success     = $vceSuccess
        exitCode    = $vceExitCode
        error       = $errorInfo
        editMode    = $editMode
    }

    $global:LASTEXITCODE = $vceExitCode
    & $global:__VceOriginalPrompt
}
