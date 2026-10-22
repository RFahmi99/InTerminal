# InTerminal shell integration. Runs once inside the user's PowerShell session (after their
# profile) and reports prompt state to the InTerminal app over a private named pipe, so the
# app never has to scrape the terminal output stream. It must work on Windows
# PowerShell 5.1 and PowerShell 7, and it must never break the user's prompt: every
# failure degrades to a plain shell with a single warning.

if (-not $env:INTERMINAL_PIPE) { return }

try {
    $__interminalPipeName = $env:INTERMINAL_PIPE -replace '^\\\\\.\\pipe\\', ''
    Remove-Item Env:\INTERMINAL_PIPE
    $__interminalPipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $__interminalPipeName, [System.IO.Pipes.PipeDirection]::Out)
    $__interminalPipe.Connect(5000)
    $global:__InTerminalWriter = New-Object System.IO.StreamWriter($__interminalPipe, (New-Object System.Text.UTF8Encoding($false)))
    $global:__InTerminalWriter.AutoFlush = $true
} catch {
    Write-Warning "InTerminal could not connect to the app ($($_.Exception.Message)). The terminal works normally, but the file tree and error help will not follow this session."
    return
} finally {
    Remove-Variable -Name __interminalPipeName, __interminalPipe -ErrorAction SilentlyContinue
}

$global:__InTerminalHasPSReadLine = [bool](Get-Command Get-PSReadLineOption -ErrorAction SilentlyContinue)
$global:__InTerminalOriginalPrompt = $function:prompt
$global:__InTerminalLastHistoryId = (Get-History -Count 1).Id
$global:__InTerminalLastError = if ($global:Error.Count) { $global:Error[0] } else { $null }

function global:__InTerminalSend($message) {
    if (-not $global:__InTerminalWriter) { return }
    try {
        $global:__InTerminalWriter.WriteLine(($message | ConvertTo-Json -Compress -Depth 4))
    } catch {
        $global:__InTerminalWriter = $null
        Write-Warning "InTerminal lost its connection to the app ($($_.Exception.Message)). The terminal keeps working without the file tree and error help."
    }
}

__InTerminalSend @{
    type      = 'hello'
    psVersion = $PSVersionTable.PSVersion.ToString()
    edition   = "$($PSVersionTable.PSEdition)"
    osVersion = [Environment]::OSVersion.VersionString
    psReadLine = $global:__InTerminalHasPSReadLine
}

function global:prompt {
    # $? and $LASTEXITCODE must be read before any other statement overwrites them.
    $interminalSuccess = $global:?
    $interminalExitCode = $global:LASTEXITCODE

    $last = Get-History -Count 1
    $ran = [bool]$last -and $last.Id -ne $global:__InTerminalLastHistoryId
    if ($last) { $global:__InTerminalLastHistoryId = $last.Id }

    $errorInfo = $null
    $newestError = if ($global:Error.Count) { $global:Error[0] } else { $null }
    if ($ran -and -not $interminalSuccess -and $newestError -and -not [object]::ReferenceEquals($newestError, $global:__InTerminalLastError)) {
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
    $global:__InTerminalLastError = $newestError

    $location = $executionContext.SessionState.Path.CurrentLocation
    $editMode = $null
    if ($global:__InTerminalHasPSReadLine) {
        try { $editMode = "$((Get-PSReadLineOption).EditMode)" } catch { $editMode = $null }
    }

    __InTerminalSend @{
        type        = 'prompt'
        cwd         = $location.ProviderPath
        provider    = $location.Provider.Name
        ran         = $ran
        commandLine = if ($ran) { $last.CommandLine } else { $null }
        success     = $interminalSuccess
        exitCode    = $interminalExitCode
        error       = $errorInfo
        editMode    = $editMode
    }

    $global:LASTEXITCODE = $interminalExitCode
    & $global:__InTerminalOriginalPrompt
}
