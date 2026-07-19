@echo off
REM ============================================================
REM  deploy.bat - push the latest code to the server's Docker.
REM
REM  Assumes the source is already on the server. This just SSHes
REM  in (over your VPN) and runs the remote deploy.sh, which
REM  rebuilds + restarts the containers.
REM
REM  Usage:
REM    deploy.bat            normal redeploy
REM    deploy.bat --logs     redeploy, then follow logs
REM    deploy.bat --no-build just restart
REM
REM  Edit HOST below to your server's VPN hostname or IP.
REM  Set up SSH key auth first (ssh-copy-id) for no password prompt.
REM ============================================================

setlocal
set "HOST=aallyn@zakros"
set "REMOTE=/opt/trove/deploy.sh"

echo Connecting to %HOST% and running deploy.sh ...
echo.
ssh %HOST% "bash %REMOTE% %*"
set "RC=%ERRORLEVEL%"

echo.
if not "%RC%"=="0" (
  echo === Deploy FAILED ^(exit %RC%^) ===
  endlocal & exit /b %RC%
)
echo === Deploy complete ===
endlocal
