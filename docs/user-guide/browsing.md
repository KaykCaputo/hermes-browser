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
| ++ctrl+w++ | Close the active tab |
| ++ctrl+tab++ | Select the next tab |
| ++ctrl+shift+tab++ | Select the previous tab |
| ++ctrl+r++ or ++f5++ | Reload the current page |
| ++alt+left++ | Go back |
| ++alt+right++ | Go forward |

## Mouse and trackpad input

The active page receives pointer movement, primary and secondary button events, and pixel-based scrolling. Navigation controls remain native UI elements outside the web content surface.

## Text input and IME

Hermes forwards physical key information, logical characters, modifier state, key repeats, and input-method composition to Servo. This supports dead keys and composed text used by many keyboard layouts and languages.

If typing works in the address bar but not inside a page, first click the page to return focus to the WebView. If composition still fails, capture the affected language, input method, and site in a bug report.

## Background tabs

Background tabs are throttled to reduce needless work. They are not yet suspended or discarded, so every open tab still keeps its WebView and browser state in memory.

There is currently no session restore. Closing Hermes discards open tabs and navigation history.

