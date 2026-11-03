# Changelog

This list is built into the app (Settings > What's new) and is posted as each GitHub release's notes, so write it for
players. Each version is `## X.Y.Z - YYYY-MM-DD` with `### New`, `### Improved` and `### Fixed` lists of `- ` items.
Changes collect under `## Unreleased` (shown only in debug builds) until the release renames it to the version.

## 0.6.0 - 2026-11-04

### New
- What's new in the settings window: what changed in each version, with a dot on it after an update.

### Fixed
- EXP/h could jump far too high, and the time to level drop to minutes, when one digit of the EXP number was misread
  for a moment. A reading counts now only when the EXP number and the percentage agree with the level's size.
- The tray icon could be missing when the overlay started with Windows (before the taskbar was ready) or after
  Explorer restarted, leaving a hidden overlay with no way back to it.
- Hide when idle: the overlay stays up for the set time after you show it, and doesn't hide while the settings
  window is open.

## 0.5.0 - 2026-10-29

### New
- A new settings window: a sidebar of pages (Overlay, Layout, Data, Capture, Effects, General, About), with a live
  preview of the overlay on the Overlay page.
- EXP/h graph item: one bar per look at the bar, to see the trend.
- Map name item: the map you're on, read once per map with the text recognition built into Windows (works with HDR).
- Fill the row: any item can take the room its row leaves.
- Updates from the settings window: it finds a new release when it opens, downloads and checks it, and offers
  Relaunch to update. It can be switched off under General.
- Effects page, with Preview effects to try them without waiting for a kill.

### Improved
- Layout editor: drop an item between rows to make a new row, and a Selected item card (Show, Fill the row, Colour)
  in place of the right-click menu.
- Quit also closes the settings window and the area picker.

### Fixed
- An empty bottom row after Windows restored the overlay.

## 0.4.1 - 2026-10-27

### Fixed
- Release signing used the right minisign build, so the in-app update could install it.

## 0.4.0 - 2026-10-27

### New
- Check for updates under Settings > About: installs only releases signed with this project's key, then restarts
  the overlay.

### Improved
- "Lv in <1 min" when less than a minute is left.
- Opening the settings again replaces a settings window that stopped answering.

### Fixed
- Win+D, Win+M or a taskbar click could minimize the overlay for good; it now stays shown and above the game.

## 0.3.0 - 2026-10-26

### New
- Effects, all off by default: rolling numbers, a +EXP pop-up, a snail per EXP gain, and rainbow text when the
  level is close.
- Preview effects in the tray menu.

### Improved
- Reads about 5 times faster, and the bar is looked at every 250 ms by default (settable).
- Nothing is captured while MapleStory isn't running.

### Fixed
- The status row sits above the layout, so it can't cover the EXP bar.

## 0.2.0 - 2026-10-25

### New
- First release: a tiny EXP/h overlay for MapleStory Classic World that reads the EXP bar from the screen, with a
  drag-and-drop layout editor, the levelling table, and window or screen capture.
