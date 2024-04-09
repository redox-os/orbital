# Orbital

The Orbital desktop environment provides a display server, window manager and compositor.

[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

<img alt="Redox" height="150" src="https://github.com/redox-os/assets/raw/master/screenshots/redox running.jpeg">

## Comparison with X11/Wayland

This display server is more simple than X11 and Wayland making the porting task more quick and easy, it's not advanced like X11 and Wayland yet but enough to port most Linux/BSD programs.

Compared to Wayland, Orbital has one server implementation, while Wayland provide protocols for compositors.

## Features

- Custom Resolutions
- App Launcher (bottom bar)
- File Manager
- Text Editor
- Calculator
- Terminal Emulator

If you hold the **Super** key (generally the key with a Windows logo) it will show all keyboard shortcuts in a pop-up.

## Libraries

The programs written with these libraries can run on Orbital.

- SDL1.2
- SDL2
- winit
- softbuffer
- Slint (through winit and softbuffer)
- Iced (through winit and softbuffer)
- egui (can use winit or SDL2)

## Clients

Apps (or 'clients') create a window and draw to it by using the [orbclient](https://gitlab.redox-os.org/redox-os/orbclient)
client.

### Client Examples

If you wish to see examples of client apps that use [orbclient](https://gitlab.redox-os.org/redox-os/orbclient)
to "talk" to Orbital and create windows and draw to them, then you can find some in [orbclient/examples](https://gitlab.redox-os.org/redox-os/orbclient/-/tree/master/examples)
folder.

## Porting

If you want to port a program to Orbital, see below:

- If the program is written in Rust probably it works on Orbital because the `winit` crate is used in most places, but there are programs that access X11 or Wayland directly. You need to port these programs to `winit` and merge on upstream.

- If the program is written in C or C++ and access X11 or Wayland directly, it must be ported to the [Orbital library](https://gitlab.redox-os.org/redox-os/liborbital).
