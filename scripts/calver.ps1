#requires -Version 7.2
param(
    [Parameter(Mandatory)]
    [ValidateSet('Prepare', 'ValidateNext', 'ValidateTag')]
    [string]$Mode,

    [string]$Tag,

    [ValidateSet('auto', 'patch', 'minor', 'major')]
    [string]$Bump = 'auto'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-TomlFieldMatches {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$Field
    )

    $pattern = '(?m)^[ \t]*' + [regex]::Escape($Field) + '[ \t]*=[ \t]*"(?<value>[^"]+)"[ \t]*(?:#.*)?\r?$'
    return ,([regex]::Matches($Text, $pattern))
}

function Get-ManifestPackageSection {
    param([Parameter(Mandatory)][string]$Text)

    $sections = [regex]::Matches($Text, '(?ms)^\[package\]\r?\n(?<body>.*?)(?=^\[|\z)')
    if ($sections.Count -ne 1) {
        throw "Expected exactly one [package] section in Cargo.toml; found $($sections.Count)."
    }

    return $sections[0]
}

function Replace-TomlField {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$Field,
        [Parameter(Mandatory)][string]$Value
    )

    $pattern = '(?m)^(?<prefix>[ \t]*' + [regex]::Escape($Field) + '[ \t]*=[ \t]*")(?<value>[^"]+)(?<suffix>"[ \t]*(?:#.*)?)(?<eol>\r?)$'
    $matches = [regex]::Matches($Text, $pattern)
    if ($matches.Count -ne 1) {
        throw "Expected exactly one '$Field' field in the selected Cargo package; found $($matches.Count)."
    }

    $replacement = '${prefix}' + $Value + '${suffix}${eol}'
    return [regex]::Replace($Text, $pattern, $replacement, 1)
}

function Get-LockPackageStanza {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$PackageName
    )

    $stanzas = [regex]::Matches($Text, '(?ms)^\[\[package\]\]\r?\n.*?(?=^\[\[package\]\]|\z)')
    $matchingStanzas = @(
        foreach ($stanza in $stanzas) {
            $names = Get-TomlFieldMatches -Text $stanza.Value -Field 'name'
            if ($names.Count -eq 1 -and $names[0].Groups['value'].Value -ceq $PackageName) {
                $stanza
            }
        }
    )

    if ($matchingStanzas.Count -ne 1) {
        throw "Expected exactly one Cargo.lock package stanza named '$PackageName'; found $($matchingStanzas.Count)."
    }

    return $matchingStanzas[0]
}

function Get-NextCalverVersion {
    param(
        [Parameter(Mandatory)][datetime]$UtcDate,
        [Parameter(Mandatory)][string]$Bump
    )

    if ($Bump -eq 'major') {
        return '{0}.1.0' -f ($UtcDate.Year + 1)
    }
    if ($Bump -eq 'minor') {
        $nextMonth = $UtcDate.AddMonths(1)
        return '{0}.{1}.0' -f $nextMonth.Year, $nextMonth.Month
    }

    $tags = @(& git tag --list)
    if ($LASTEXITCODE -ne 0) { throw "git tag --list failed: $LASTEXITCODE" }

    $tagPattern = '^v(?<year>[0-9]{4})\.(?<month>0|[1-9][0-9]*)\.(?<patch>0|[1-9][0-9]*)$'
    $highestPatch = [System.Numerics.BigInteger]::Zero
    $foundCurrentMonthTag = $false

    foreach ($candidateTag in $tags) {
        if ($candidateTag -cnotmatch $tagPattern) { continue }

        $tagYear = [int]$Matches['year']
        $tagMonth = [int]$Matches['month']
        if ($tagMonth -lt 1 -or $tagMonth -gt 12) { continue }
        if ($tagYear -ne $UtcDate.Year -or $tagMonth -ne $UtcDate.Month) { continue }

        $patch = [System.Numerics.BigInteger]::Parse(
            $Matches['patch'],
            [System.Globalization.CultureInfo]::InvariantCulture
        )
        if (-not $foundCurrentMonthTag -or $patch -gt $highestPatch) {
            $highestPatch = $patch
            $foundCurrentMonthTag = $true
        }
    }

    $nextPatch = if ($foundCurrentMonthTag) { $highestPatch + [System.Numerics.BigInteger]::One } else { [System.Numerics.BigInteger]::Zero }
    return '{0}.{1}.{2}' -f $UtcDate.Year, $UtcDate.Month, $nextPatch.ToString([System.Globalization.CultureInfo]::InvariantCulture)
}

if ($Mode -eq 'ValidateTag' -and [string]::IsNullOrWhiteSpace($Tag)) {
    throw '-Tag is required when Mode is ValidateTag.'
}

Push-Location (Join-Path $PSScriptRoot '..')
try {
    $manifestPath = Join-Path (Get-Location) 'Cargo.toml'
    $lockPath = Join-Path (Get-Location) 'Cargo.lock'
    $manifestText = [System.IO.File]::ReadAllText($manifestPath)
    $lockText = [System.IO.File]::ReadAllText($lockPath)

    $manifestSection = Get-ManifestPackageSection -Text $manifestText
    $nameMatches = Get-TomlFieldMatches -Text $manifestSection.Groups['body'].Value -Field 'name'
    $manifestVersionMatches = Get-TomlFieldMatches -Text $manifestSection.Groups['body'].Value -Field 'version'
    if ($nameMatches.Count -ne 1 -or $manifestVersionMatches.Count -ne 1) {
        throw 'The [package] section must contain exactly one name and one version field.'
    }

    $packageName = $nameMatches[0].Groups['value'].Value
    $manifestVersion = $manifestVersionMatches[0].Groups['value'].Value
    $lockStanza = Get-LockPackageStanza -Text $lockText -PackageName $packageName
    $lockVersionMatches = Get-TomlFieldMatches -Text $lockStanza.Value -Field 'version'
    if ($lockVersionMatches.Count -ne 1) {
        throw "Expected exactly one version field in the Cargo.lock package stanza for '$packageName'."
    }

    $lockVersion = $lockVersionMatches[0].Groups['value'].Value
    if ($manifestVersion -cne $lockVersion) {
        throw "Cargo.toml version '$manifestVersion' does not match the root Cargo.lock version '$lockVersion'. Refusing to prepare or release."
    }

    $versionPattern = '^[0-9]{4}\.(?:[1-9]|1[0-2])\.(?:0|[1-9][0-9]*)$'
    if ($manifestVersion -cnotmatch $versionPattern) {
        throw "Cargo version '$manifestVersion' is not a valid CalVer version (YYYY.M.patch, with an unpadded month)."
    }

    $nextVersion = Get-NextCalverVersion -UtcDate ([datetime]::UtcNow) -Bump $Bump

    switch ($Mode) {
        'Prepare' {
            $updatedManifestSection = Replace-TomlField -Text $manifestSection.Groups['body'].Value -Field 'version' -Value $nextVersion
            $updatedManifest = $manifestText.Substring(0, $manifestSection.Groups['body'].Index) +
                $updatedManifestSection +
                $manifestText.Substring($manifestSection.Groups['body'].Index + $manifestSection.Groups['body'].Length)

            $updatedLockStanza = Replace-TomlField -Text $lockStanza.Value -Field 'version' -Value $nextVersion
            $updatedLock = $lockText.Substring(0, $lockStanza.Index) +
                $updatedLockStanza +
                $lockText.Substring($lockStanza.Index + $lockStanza.Length)

            $encoding = [System.Text.UTF8Encoding]::new($false)
            if ($updatedManifest -cne $manifestText) {
                [System.IO.File]::WriteAllText($manifestPath, $updatedManifest, $encoding)
            }
            if ($updatedLock -cne $lockText) {
                [System.IO.File]::WriteAllText($lockPath, $updatedLock, $encoding)
            }

            Write-Output $nextVersion
        }
        'ValidateNext' {
            if ($manifestVersion -cne $nextVersion) {
                $recoveryTag = "v$manifestVersion"
                $tagCommit = @(& git rev-parse --verify "refs/tags/$recoveryTag^{commit}" 2>$null)
                $tagExitCode = $LASTEXITCODE
                if ($tagExitCode -ne 0 -or $tagCommit.Count -ne 1) {
                    throw "Cargo version '$manifestVersion' is stale; the next available CalVer version is '$nextVersion'. Run the Release workflow on the default branch to prepare the next version."
                }

                $headCommit = @(& git rev-parse --verify HEAD)
                if ($LASTEXITCODE -ne 0 -or $headCommit.Count -ne 1 -or $tagCommit[0] -cne $headCommit[0]) {
                    throw "Cargo version '$manifestVersion' is not the next available CalVer version '$nextVersion', and its existing recovery tag '$recoveryTag' does not point to the checked-out commit. Run the Release workflow on the default branch to prepare the next version."
                }
            }
            Write-Output $manifestVersion
        }
        'ValidateTag' {
            $expectedTag = "v$manifestVersion"
            if ($Tag -cne $expectedTag) {
                throw "Release tag '$Tag' does not match the checked-out Cargo version '$manifestVersion' (expected '$expectedTag')."
            }

            $tagCommit = @(& git rev-parse --verify "refs/tags/$Tag^{commit}")
            if ($LASTEXITCODE -ne 0 -or $tagCommit.Count -ne 1) {
                throw "Could not resolve release tag '$Tag' to a commit."
            }
            $headCommit = @(& git rev-parse --verify HEAD)
            if ($LASTEXITCODE -ne 0 -or $headCommit.Count -ne 1) {
                throw 'Could not resolve the checked-out commit.'
            }
            if ($tagCommit[0] -cne $headCommit[0]) {
                throw "Release tag '$Tag' does not point to the checked-out commit; refusing to build a release from a different revision."
            }

            Write-Output $manifestVersion
        }
    }
}
finally {
    Pop-Location
}
