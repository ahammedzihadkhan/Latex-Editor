@echo off
taskkill /f /im inkTeX.exe >nul 2>&1
taskkill /f /im latex-ai-editor.exe >nul 2>&1
echo inkTeX stopped.
timeout /t 2 >nul

