@echo off
echo Initializing Visual Studio C++ Environment...
call "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
echo.
echo Running LiteClip Replay...
set RUST_BACKTRACE=1
cargo run %*
