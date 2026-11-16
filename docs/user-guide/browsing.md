---
title: Tabs and Shortcuts
description: Keyboard shortcuts, navigation controls, tab behavior, and text input in Mercury Browser.
---

# Tabs and shortcuts

Mercury follows familiar desktop-browser shortcuts while keeping the interface intentionally compact.

## Keyboard reference

| Shortcut | Action |
| --- | --- |
| ++ctrl+l++ | Focus and select the address bar |
| ++ctrl+t++ | Open a new tab |
| ++ctrl+w++ | Close the active tab |
| ++ctrl+tab++ | Select the next tab |
| ++ctrl+shift+tab++ | Select the previous tab |
| ++ctrl+r++ or ++f5++ | Reload the current page |
| ++alt+left++ | Go back |
| ++alt+right++ | Go forward |

On macOS, ++cmd++ takes the place of ++ctrl++, `Cmd+[` and `Cmd+]` go back and forward, and ++cmd+q++ quits.

## Menus

On macOS, Mercury has a menu bar with Mercury Browser, File, Edit, View, History, Bookmarks, Window and Help menus. Each menu item shows its keyboard shortcut, and the Window menu lists the open tabs. The ⋮ button at the end of the toolbar opens the browser menu on every platform. It has New Tab, zoom controls showing the current zoom level, Settings, About Mercury Browser, Help and Quit; the arrow keys move through it, Return chooses and Escape closes it. Find appears greyed out because it is not available yet, as do the Bookmarks menu items. The menus follow the appearance setting.

**About Mercury Browser** shows the version, the engine, the licence and the third-party notices.

## Mouse and trackpad input

The active page receives pointer movement, primary and secondary button events, and pixel-based scrolling. Navigation controls remain native UI elements outside the web content surface.

## Text input and IME

Mercury forwards physical key information, logical characters, modifier state, key repeats, and input-method composition to Servo. This supports dead keys and composed text used by many keyboard layouts and languages.

If typing works in the address bar but not inside a page, first click the page to return focus to the WebView. If composition still fails, capture the affected language, input method, and site in a bug report.

## Background tabs

Background tabs are throttled to reduce needless work. They are not yet suspended or discarded, so every open tab still keeps its WebView and browser state in memory.

There is currently no session restore. Closing Mercury discards open tabs and navigation history.

