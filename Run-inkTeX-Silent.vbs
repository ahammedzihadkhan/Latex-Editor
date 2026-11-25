Set fso = CreateObject("Scripting.FileSystemObject")
currentDir = fso.GetParentFolderName(WScript.ScriptFullName)
exePath = currentDir & "\inkTeX.exe"

Set WshShell = CreateObject("WScript.Shell")
WshShell.Run Chr(34) & exePath & Chr(34), 0, False

