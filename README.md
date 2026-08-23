# FPS Unlocker

Menu bar app for macOS that lets you unlock the FPS of Roblox. This was ripped out of the [Alterra External](https://www.alterrasoftware.site/) and modified to keep working after Roblox updates.

## How to install

```sh
curl -fsSL "https://raw.githubusercontent.com/imeowforcash/fps-unlocker/main/installer.sh" | bash
```

## How to use

Close Roblox if it's open, then press "Resign Roblox", it will be greyed out if it's already resigned, you will have to resign after Roblox updates or after reinstalling it. After resigning open Roblox and press "Unlock FPS" and that's it.

## How it works

It attaches to Roblox, scans for the scheduler's frame interval and the Metal display sync flag.

The scheduler is found by walking Roblox's image for pointers into the heap that look like a job list. The FPS cap is usually the last interval looking field on that object. 

Display sync is found by flipping it on a dummy CAMetalLayer and looking at which bit moved, then looking for that same bit on Roblox's layer.

## Is this bannable?

No. Roblox's anti-cheat doesn't even try to detect this at all, especially on Mac.

## Why Tauri?

I started with Tauri because I thought I would make a really good frontend for it, but I got lazy, and I was already halfway done so I didn't wanna move away from Tauri, so I just threw out the frontend and decided to make it a menu bar app.
