# Changelog

Each release's notes on GitHub come from its section here (`## X.Y.Z`).

## 0.3.7

- Windows in Vietnamese: follows the Windows display language, or Settings › General › Language (System, English, Tiếng Việt). It uses the Mac app's translations plus the Windows-only text.
- Windows: Developer Tools page (Docker, and Android system images and older NDKs through sdkmanager).
- Windows: Settings › Cleaning › Project folders, so Project Junk also looks on other drives (D:\ and others).
- Windows: alerts in Settings › Automation when the system drive is almost full, or the Recycle Bin or Downloads grows past a size; at most once a day each.
- Dashboard (macOS): the Trash is measured again only when it changed.
- macOS: a welcome on first launch with the permissions OpenCleaner may need and the menu bar option.
- Uninstaller: an open app has a Quit button right in its row, and sizes are what removing frees (apps that share files, like ChatGPT and Codex, are no longer counted twice).
- Slim Apps remembers apps whose signature would break and leaves them unticked, saying why, until they update.
- Dashboard: shows what you freed this month (opens History), and "Empty the Trash" opens System Junk on the Trash alone.
- Touch ID for sudo: no empty button bar on Macs that cannot use it.
- Developer Tools shows how much the last System Junk scan found in developer caches; one click opens System Junk limited to that group (scanning first if needed), and Clean then acts only on what is shown.
- History: open a session to see what was skipped or could not be removed, and why (an app that was running, one that needs administrator rights…).
- System Junk removes caches that keep read-only folders (dotslash, Go modules) instead of failing on them every time.
- Developer Tools: Android emulator system images no emulator uses, and NDKs older than the newest one, removed with Android's own sdkmanager. Never ticked for you.

## 0.3.6

- System Junk › Developer tools covers editors and AI coding apps: VS Code (and Insiders), Cursor, Windsurf, Antigravity, Kiro, Trae, Qoder, Void, VSCodium, Positron, CodeBuddy, Zed, ChatGPT and Codex (including downloaded update packages), the Codex CLI cache, Claude, Orca, Factory, Superset, Sculptor, Replit, LM Studio, Jan and Msty, and JetBrains IDE and Android Studio indexes and logs. Each app has its own row and is skipped while it runs; projects, chats, models and settings are never touched. Windows gets the editors too.
- Developer Tools points to those caches in System Junk.
- macOS: the Dashboard's Trash figure is what emptying it frees, as System Junk counts it. App copies that share their space with the apps in Applications no longer show as gigabytes to free.
- macOS: lists use more of the window: pages start right under the title bar, and the Uninstaller's app list runs to the bottom until you pick an app.
- macOS: scroll bars in every list stay off the panel's rounded corners.
- macOS: Installers counts what removing a file frees: a cloned copy of an installer no longer shows its full size.
- macOS: the Dashboard's "Clean … of junk" suggestion shows what System Junk removes by default, not everything it found (Trash and privacy items are never ticked for you).

## 0.3.5

- macOS and Windows: the window opens at a more compact 1280 x 880 (the whole sidebar still fits) instead of most of the screen.
- macOS: Settings > Permissions offers "Reopen OpenCleaner" while Full Disk Access shows Not allowed: macOS applies that permission only when the app starts again.

## 0.3.4

**Windows**
- Project Junk, Large & Old Files, Installers and Duplicates now find files in folders synced by OneDrive (Desktop, Documents, Pictures…). Files that are only in the cloud are left out: removing them frees nothing on this PC.
- Project Junk looks through your whole user folder (except AppData).
- Updates install themselves: OpenCleaner downloads the new version with a progress bar you can cancel, checks it against the release's SHA-256, swaps it in and restarts. Works from 0.3.4 on, so the step from 0.3.3 is still by hand.
- Smoother page switching (no more flicker).
- The Mac illustrations on Dashboard, empty lists, the Privacy group, Space Lens and Settings > Automation.
- The window opens at most of the screen.

**macOS**
- Space Lens no longer lists mounted disk images (.dmg) as disks.
- Settings: switches and menus in the app's own style instead of the system blue.
- New colors for History (plum) and Settings (deep teal); the sidebar scroller stays off the panel's corners.
- The window opens at most of the screen, so the whole sidebar fits on large displays.

## 0.3.3

- Windows: Desktop, Documents and the other user folders are found where Windows really keeps them; Project Junk also looks in Android Studio, Flutter and Visual Studio project folders.
- Windows: quicker page fade, an icon on every Space Lens bubble, stronger colors in the light theme.
- macOS: Settings sections line up with the tabs.

## 0.3.2

- macOS: Settings is a page of the main window with tabs (⌘, opens it), no separate window.
- Windows: Settings in tabs like macOS (updates, excluded folders, Large & Old Files limits, dashboard refresh).
- Windows: motion like the Mac (page fade, growing meters, Space Lens bubbles that spring in), off when Windows' animation effects are off.
