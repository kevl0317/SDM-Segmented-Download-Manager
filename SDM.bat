@echo off
rem SDM launcher -- double-click to open the download manager (no console window).
setlocal
set "PY=F:\miniconda\python.exe"
if not exist "%PY%" set "PY=python"
set "PYW=F:\miniconda\pythonw.exe"
if not exist "%PYW%" set "PYW=pythonw"
set "APP=%~dp0dist\sdm.pyz"
if not exist "%APP%" (
  echo Building sdm.pyz ...
  "%PY%" "%~dp0build.py" || goto :error
)
start "" "%PYW%" "%APP%" %*
exit /b 0
:error
echo Build failed. Run "%PY% build.py" manually to see the error.
pause
