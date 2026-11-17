# GameSir T3 Lite L4/R4 Remapper

A tiny Windows tool that turns the **L4 / R4 back buttons** of the **GameSir T3 Lite** into **keyboard keys or shortcuts** (for example, `E`, `Space` or `Ctrl+C`).

Officially can only map the back buttons to other *controller* buttons. This tool reads them directly from the controller and sends real keyboard input instead.

> **Unofficial project.** Not affiliated with, endorsed by, or supported by GameSir. "GameSir" is a trademark of its respective owner.

## Features

- Map **L4** and **R4** to any key, a modifier alone (e.g. `Shift`), or a combination (e.g. `Ctrl + Shift + Tab`)
- Live indicators for controller connection and L4/R4 state
- Optional **auto-repeat while held** (useful for typing; games usually don't need it)
- Runs in the **system tray**, with the window opening only when you need it
- Lightweight: written in Rust, single `.exe`, no installer, no runtime
- Settings saved in a `config.json` **next to the executable** (portable)
- Detects the controller automatically and reconnects when you replug it

## How it works

The T3 Lite exposes a vendor-defined HID collection (usage page `0xFFF0`) that GameSir Connect uses for its configuration and test screens. L4/R4 do not appear in the normal XInput report, but they are reported on this channel:

| Byte index | Mask   | Button |
|------------|--------|--------|
| 14         | `0x04` | L4     |
| 14         | `0x08` | R4     |

The tool opens that collection with [`hidapi`](https://crates.io/crates/hidapi), watches that byte, and injects keyboard events with the Windows `SendInput` API (using scan codes, so most games accept them).

This was found by reverse engineering one unit (VID `0x3537`, PID `0x1093`). Other firmware revisions may report things differently.

## Requirements

- Windows 10 or 11 (x64)
- GameSir T3 Lite connected by USB
- **L4/R4 left unassigned on the controller itself** (otherwise the game also receives the copied button)

## Usage

1. Download the latest `.zip` from the [Releases](../../releases) page and extract it anywhere you can write to (not `C:\Program Files`).
2. Run `gamesir-remap.exe`. The window opens and a green icon appears in the system tray.
3. Press **Define key / shortcut** next to L4 or R4, then press the key or combination you want.
4. Close the window whenever you like. The app keeps running in the tray.

Tray icon:

- **Double-click** or **right-click → Open window**: open the window again
- **Right-click → Exit**: quit completely

### Command line modes

| Argument  | Behavior                                                        |
|-----------|-----------------------------------------------------------------|
| *(none)*  | Start the background process and open the window                |
| `--tray`  | Start the background process only, without opening a window     |
| `--gui`   | Window only (used internally; closing it ends only that process) |

### Start with Windows

Press `Win + R`, type `shell:startup`, and put a shortcut to `gamesir-remap.exe` in that folder. In the shortcut's properties, add ` --tray` to the end of the **Target** field so no window opens at login.

## Configuration

Settings live in `config.json` next to the executable. You normally don't need to edit it by hand. Keys are stored as [Windows virtual-key codes](https://learn.microsoft.com/windows/win32/inputdev/virtual-key-codes) and pressed in order, so `Ctrl+C` looks like this:

```json
{
  "buttons": [
    { "keys": [17, 67], "repeat": false },
    { "keys": [], "repeat": false }
  ]
}
```

The first entry is L4, the second is R4. The background process re-reads the file on every button press, so changes apply immediately.

## Building from source

You need the [Rust toolchain](https://rustup.rs) and, on Windows, the **Visual Studio Build Tools** with the "Desktop development with C++" workload.

```powershell
git clone https://github.com/Silenc3rz/gamesir-t3-lite-backbuttons-remap.git
cd gamesir-t3-lite-backbuttons-remap
cargo build --release
```

The executable will be at `target\release\gamesir-remap.exe`.

## Known limitations

- **Windows only.** It uses the Win32 API for keyboard input and the tray.
- **Games running as administrator** ignore input from non-elevated programs. Run this tool as administrator in that case.
- **Anti-cheat.** Some competitive games treat software-injected input as macros or cheating. Check the game's rules before using it there. Use at your own risk.
- **Unsigned executable.** Windows SmartScreen or your antivirus may warn about it, since the app reads a HID device and injects keystrokes. You can read the source and build it yourself.
- Only tested on a single T3 Lite unit.
- If the window is already open, "Open window" does nothing (it doesn't bring it to the front).
- Don't press L4/R4 while defining a key, or the injected key may be captured instead of the one you wanted.
- Exiting while L4/R4 is held may leave the mapped key stuck. Release the button first.

## Troubleshooting

**The indicator says "Controller not found".** Make sure the controller is plugged in via USB and appears in Device Manager. Close GameSir Connect and try again.

**L4/R4 never light up.** Your firmware may use a different report layout. Open an issue with your controller's VID/PID and, if you can, a capture of the packets sent when you press L4 and R4.

**The key is sent twice (or the game sees an extra button).** L4/R4 are still assigned on the controller. Clear them (hold `M` + `L4` / `R4`, then press `L4` / `R4` again without choosing a button).

## Contributing

Issues and pull requests are welcome, especially reports from other T3 Lite units or firmware versions.

## License

[MIT](LICENSE)
