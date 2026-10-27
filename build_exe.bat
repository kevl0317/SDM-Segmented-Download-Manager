@echo off
rem Build a standalone Windows .exe -- the result needs no Python installed.
rem First run installs PyInstaller (needs internet once).
setlocal
set "PY=F:\miniconda\python.exe"
if not exist "%PY%" set "PY=python"
"%PY%" -m pip install --upgrade pyinstaller || goto :error
"%PY%" -m PyInstaller --noconfirm --onefile --windowed --name SDM ^
    --collect-submodules sdm "%~dp0run_app.py"
echo.
echo Done. Standalone executable: dist\SDM.exe
exit /b 0
:error
echo Could not install PyInstaller. Check your internet connection / pip.
pause
