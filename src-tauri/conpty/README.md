# Bundled ConPTY (Windows only)

`conpty.dll` and `OpenConsole.exe` (x64) from Microsoft's
[Microsoft.Windows.Console.ConPTY](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY)
NuGet package, version 1.25.261109001, MIT licensed.

portable-pty loads `conpty.dll` from next to `myterm.exe` in preference to the
one built into Windows. The built-in console host stalls every new terminal for
about 3 seconds at start-up on recent Windows 11 builds; this one does not.

`build.rs` copies both files next to the built exe, and `tauri.windows.conf.json`
puts them in the installers. To update, take the same two files from a newer
package version (`runtimes/win-x64/native/conpty.dll` and
`build/native/runtimes/x64/OpenConsole.exe`).
