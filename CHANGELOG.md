# Changelog

Each release's notes on GitHub come from its section here (`## X.Y.Z`).

## 0.3.15

- macOS: System Junk and file review lists now use one compact group-filter dropdown instead of horizontally scrolling chips. The button shows the current group; open it to choose a group or return to All groups.
- Each menu entry includes its item count and size, with localized labels and accessibility text. Long selected names stay within the filter button instead of overlapping Select All.
- Group filtering is also available for review lists with more than 20 groups, including large duplicate-file lists.
- Administrator processes and moves to the Trash wait on Dispatch queues, keeping blocking I/O out of Swift's cooperative task pool when several operations run together.
- Validation: UI responsiveness tests use a process handshake rather than a wall-clock speed threshold, and hard-link fixtures flush their data before checking APFS private allocation.

## 0.3.12

**macOS**

- Section chips stay inside their horizontal scroll area instead of overlapping Select All or the sidebar.
- Dashboard updates live metrics independently of the Trash measurement, so a large Trash no longer keeps Refresh disabled. Returning to the Dashboard takes a fresh reading.
- Space Lens and Dashboard use the same immediately available disk capacity. Space Lens refreshes capacity on entry, focus and rescan, and rechecks cached folder listings while showing the previous result as old.
- Space Lens scans only the drive or folder you open, cancels scans when you leave, and prevents an older scan from replacing newer results. Cache reads and writes and moves to the Trash run off the UI thread; moving multiple items shows progress and keeps individual failures visible.
- Optimization explains each task's benefit and trade-off and leaves every task unselected until you choose it. Administrator commands no longer block the UI, and each task reports its own result and error instead of a later success hiding an earlier failure.
- Developer Tools explains why an image or runtime is considered unused and warns that older NDKs may still be pinned by projects. Last-use dates are shown as not recorded when the scan has no reliable usage history.

**Release checks**

- Publishing now waits for the engine checks as well as both application builds.

## 0.3.11

- macOS: new Process Doctor (Speed) shows which of your processes are stuck in a loop, using a lot of memory or left running, and which apps hold the most memory. Each can be quit: the app is asked to quit first, as closing it would, and Force Quit is offered only if it does not. Only your own processes are listed; system processes, Finder, Dock and the app's own parents are never offered.
- Command line: `oce hogs [--json]` lists them and `oce quit [--force] PID` quits one, with the same checks (a reused pid is refused). The engine reads processes on Windows too; the Windows page is not built yet.

## 0.3.10

- macOS: an app counts as open only while its own executable runs. Tools inside an app no longer keep its junk from being cleaned (sourcekit-lsp and DeviceHub inside Xcode.app kept DerivedData skipped while Xcode was closed).
- System Junk measures what running apps hold too, and each such row has Quit [app]: the app is asked to quit (it can still save), then the scan runs again and the row can be cleaned. What running apps hold is not counted in the total.

- Long review lists (Hidden Items, Installers, Project Junk, Large & Old Files, System Junk) have a chip per section, with its count and size, to show one section at a time instead of scrolling to it (Windows: the pick lists).
- Select All / Deselect All for the rows shown. When some of them were left unticked on purpose, a warning says what they may hold before they are ticked.

## 0.3.9

- Hidden Items gives each item a verdict: Safe to remove, Probably outdated, Still needed or Not recognized, with what it holds (a cache downloaded again when needed, the tool itself, or settings and data), whether its app is still installed, and when anything inside it was last used (the whole folder, not only its top level).
- Hidden Items looks inside tools' folders for older versions kept next to the newer one: Gradle distributions and caches, Node.js (nvm), Ruby (rbenv), Python (pyenv), Rust toolchains, Kotlin/Native, extensions of VS Code, Cursor, Windsurf, Kiro, Trae and Antigravity, and Flutter packages in .pub-cache. The newest and the one set as default stay; older downloads installed 90 days ago or more are ticked (older Node, Ruby and Python versions are only listed: projects and venvs may use them).
- Windows: text made of parts ("… · last used 3 days ago") is translated part by part.

## 0.3.8

- macOS: installing an update shows its download in a small window with a progress bar and Cancel, then "Checking and installing…", as on Windows.
- Hidden Items (macOS and Windows): the hidden folders and files in your home folder, each with what it belongs to. Old backups (`.zshrc.backup.…`) and files zsh rebuilds are ticked; download caches of tools that are no longer installed are ticked once unchanged for 90 days; the rest is only shown: in use, left by removed apps, unchanged for 6 months or not recognized. Known settings, keys and history are never listed. Everything goes to the Trash.
- Windows: a Microsoft Store package (MSIX) is built with each release; the Store copy is updated by the Store.

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
