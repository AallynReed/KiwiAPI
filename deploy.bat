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
REM  HOST is the ssh_config alias ZakrosL (root@10.0.0.253:22, LAN).
REM  ALWAYS use ZakrosL - the other aliases do not work for deploys:
REM  bare "zakros" does not resolve, AZakrosL (aallyn@) is denied by
REM  publickey, and Zakros (via the VPS on :1511) is ipset-restricted.
REM ============================================================

setlocal
set "HOST=ZakrosL"
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
