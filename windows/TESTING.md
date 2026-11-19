# Testing OpenCleaner for Windows by hand

CI builds the app, runs the unit tests and a smoke test: it starts `OpenCleaner.exe`, checks
that the window shows, opens every page from the keyboard and saves screenshots (the
`OpenCleaner-windows-screenshots` artifact of the `windows-app` job). It never clicks, cleans or
removes anything. Use this list on a real PC before a release. Use a test user account, or
at least a folder you can lose, for the steps that remove files.

Write down the Windows version (`winver`), the display scale and the theme with each report.

## Start

- [ ] Download `OpenCleaner.exe` from the release or CI artifact and double-click it. SmartScreen
      may warn (the exe is unsigned): More info, Run anyway.
- [ ] The window opens within a few seconds, about 1080 x 720, with the Dashboard page.
- [ ] The title bar takes the page's color (Windows 11) and its text reads "OpenCleaner - Dashboard".
- [ ] Task Manager shows one OpenCleaner process, using almost no CPU once the dashboard has loaded.

## Every page

Open each page from the sidebar. Nothing overlaps, no text is cut off mid-word in a way that hides
its meaning, and the page art shows.

- [ ] Dashboard: CPU, memory, disk and Recycle Bin tiles fill in; top processes list; the
      refresh button and F5 read new numbers.
- [ ] System Junk: intro, Scan button.
- [ ] Installers, Project Junk, Large & Old Files, Duplicates: intro, Scan button.
- [ ] Uninstaller: the installed apps list, sizes fill in, refresh button.
- [ ] Startup Items: your startup entries (HKCU Run, your Startup folder) with Turn Off; the
      machine's entries (HKLM Run, the all-users Startup folder) show "System, needs admin" and
      a lock, with no button; entries switched off in Task Manager show "Off".
- [ ] Space Lens: This PC and Your user folder cards.
- [ ] History: past cleanups, newest first, and the path of the deletion log.
- [ ] Settings: three panels (Engine, Dashboard, Automation), all visible without scrolling at
      the smallest window size.

## Scan and clean

- [ ] System Junk: Scan. The size found grows while scanning, Esc stops it. A finished scan lists
      groups (Browsers, Developer tools, Temporary files & logs, Privacy) with the default rules
      ticked. Nothing has been removed yet.
- [ ] The Privacy group (browser history and cookies) is never ticked, says that removing cookies
      signs you out, and each rule says "Skipped while ... is open" while that browser runs.
- [ ] Open a rule with its chevron, untick an item, open a folder item and untick a child: the
      rule's box shows a dash and the size at the bottom drops.
- [ ] Clean: a confirmation with the size; Cancel leaves everything; OK shows the removal
      progress bar (item being removed, count, bytes freed), then the outcome panel.
- [ ] Create a test folder with a few large files (for example `fsutil file createnew big1.bin
      600000000` in Downloads, then set its date back or wait) and run Large & Old Files and
      Duplicates (copy one file twice): they are found, Remove asks first and sends them to the
      Recycle Bin, where they can be restored.
- [ ] Project Junk: a test project with `node_modules` or `target` is found; Delete asks first.
- [ ] Space Lens: open This PC, then a folder; the bubbles and the list agree; tick a test file,
      Remove asks first and the file lands in the Recycle Bin.
- [ ] History lists each of these runs.

## Startup Items

- [ ] Add a test entry: `reg add HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v OCTest /d
      "C:\Windows\notepad.exe"`, refresh (button or F5): "notepad" is listed.
- [ ] Turn Off asks first (says the registry entry is removed and the command is kept in the
      history); Cancel keeps it; OK removes it from the list and from the Run key (`reg query`).
- [ ] Put a shortcut in `shell:startup`, refresh, Turn Off: the shortcut goes to the Recycle Bin.
- [ ] As a standard user, HKLM entries have no Turn Off button.
- [ ] Open Startup Apps opens Windows Settings > Apps > Startup.

## Settings

- [ ] Faster scans and Refresh when the app comes to the front switch on and off, and are kept
      after closing and reopening the app (HKCU\Software\OpenCleaner).
- [ ] Clean automatically is off on a fresh install; Weekly / Monthly can only be changed while
      it is on.
- [ ] Scheduled run: turn Clean automatically on, close the app, delete the `AutoCleanLast` value
      (`reg delete HKCU\Software\OpenCleaner /v AutoCleanLast /f`), start the app. Within a minute
      a notification says what was freed, Settings shows "Last run ..." and History has a new
      System Junk entry. Only rules ticked by default were cleaned: browser history, cookies and
      the Recycle Bin are untouched.
- [ ] Restart the app again: it does not clean again (not due for a week / month).
- [ ] With Clean automatically off, deleting `AutoCleanLast` and restarting cleans nothing.

## Theme

- [ ] Settings > Personalization > Colors > app mode Light, then Dark while the app runs: the app
      follows at once (text, glass, title bar), on every page.

## Keyboard

- [ ] Tab and Shift+Tab move a visible focus ring along the sidebar and every button; Enter or
      Space activates; Up/Down move along tick boxes and scroll the list to keep the focus in view.
- [ ] Enter with nothing focused presses the big round button.
- [ ] Ctrl+R and F5 scan again (or refresh), Esc stops a scan.
- [ ] A screen reader (Narrator) announces the page through the window title.

## Display scaling

- [ ] At 100%, 150% and 200% (Settings > System > Display > Scale): text is sharp (not blurry
      bitmap scaling), nothing overlaps, the sidebar fits with the engine version at the bottom.
- [ ] Drag the window between monitors with different scales: it resizes and stays sharp.
- [ ] Resize to the smallest size: every page still fits, Settings included.

## Windows 10 and Windows 11

Run the whole list once on each.

- [ ] Windows 11: rounded corners, colored title bar, Segoe Fluent Icons glyphs.
- [ ] Windows 10 (1809 or later): square corners and the system title bar are expected; the
      sidebar and button glyphs still show (Segoe MDL2 Assets), not empty boxes.
- [ ] The notification of a scheduled clean shows as a toast on both.

## After a crash

- [ ] If the app ever quits unexpectedly, the next start offers to report it on GitHub, with the
      crash log from `%LOCALAPPDATA%\OpenCleaner\crash.log`.
