---
title: Tabs and Shortcuts
description: Keyboard shortcuts, navigation controls, tab behavior, and text input in Hermes Browser.
---

# Tabs and shortcuts

Hermes follows familiar desktop-browser shortcuts while keeping the interface intentionally compact.

## Keyboard reference

| Shortcut | Action |
| --- | --- |
| ++ctrl+l++ | Focus and select the address bar |
| ++ctrl+t++ | Open a new tab |
| ++ctrl+d++ | Save or remove the current bookmark |
| ++ctrl+shift+t++ | Reopen the most recently closed tab |
| ++ctrl+1++ through ++ctrl+8++ | Select a tab by position |
| ++ctrl+9++ | Select the last tab |
| Ctrl and + / − / 0 | Zoom in, zoom out, or reset page zoom |
| ++f11++ | Toggle fullscreen |
| ++ctrl+q++ | Quit and save open tabs |
| ++ctrl+w++ | Close the active tab |
| ++ctrl+tab++ | Select the next tab |
| ++ctrl+shift+tab++ | Select the previous tab |
| ++ctrl+r++ or ++f5++ | Reload the current page |
| ++alt+left++ | Go back |
| ++alt+right++ | Go forward |

## Mouse and trackpad input

The active page receives pointer movement, primary and secondary button events, and pixel-based scrolling. Links that request a new window after a click or keyboard activation open in a new tab. Video players can request fullscreen. Escape returns to the page.

## Text input and IME

Hermes forwards physical key information, logical characters, modifier state, key repeats, and input-method composition to Servo. This supports dead keys and composed text used by many keyboard layouts and languages.

If typing works in the address bar but not inside a page, first click the page to return focus to the WebView. If composition still fails, capture the affected language, input method, and site in a bug report.

## Background tabs

Restoring a session loads the selected page first. Each other saved tab starts loading when you select it. Previously visited background tabs keep their WebViews and are throttled.

Hermes saves the open tab addresses, their order, and the selected tab on normal exit. Reopening a tab loads its address again. Select a saved bookmark from the new-tab page.


## Customize the browser

Use the toolbar menu to switch between horizontal and vertical tabs, reopen the last closed tab, change zoom, or toggle fullscreen. The arrow at the top of the vertical tab strip collapses it to icons. Drag a tab to reorder it and right-click to pin it. Pinned tabs stay together and are restored with the session.

The address bar suggests matching bookmarks and open tabs. Use the arrow keys and Enter to pick a result, or Escape to dismiss the list. These suggestions come from the current window and saved bookmarks.

Open **Customize** on the new-tab page to switch between System, Light, and Dark themes, or choose a solid or gradient background. Use the toolbar star to add a shortcut and right-click a shortcut to remove it.

A tab playing media shows a control to mute HTML video or audio in the page and its same-origin frames. The control depends on the playback state reported by Servo.
