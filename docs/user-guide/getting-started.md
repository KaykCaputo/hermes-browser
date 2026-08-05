---
title: Start Browsing
description: Open pages, search with DuckDuckGo, and understand the basic Hermes browsing workflow.
---

# Start browsing

Hermes opens with a clean new tab. The address bar accepts both web addresses and ordinary search terms, so there is no separate search box to learn.

![The Hermes new tab page](../screenshots/hermes-new-tab.png)

## Open a website

Enter any of these forms in the address bar and press ++enter++:

```text
https://servo.org/
servo.org
localhost:8080
127.0.0.1:3000
```

When a hostname has no scheme, Hermes adds `https://` automatically. Explicit `http://`, `https://`, and `about:` addresses are supported.

## Search the web

Enter words that do not look like a hostname:

```text
how does the Servo web engine work
```

Hermes sends the query to DuckDuckGo. Search is a normal navigation, so the resulting page appears in the tab's history.

## Read the navigation state

While a page is loading, Hermes keeps the address, title, icon, history controls, and loading feedback in sync with Servo. Back and forward buttons become available only when that direction exists in the tab's history.

If a title changes but the page remains blank, the network and script stages may have succeeded while painting failed. See [A page stays blank](troubleshooting.md#a-page-stays-blank).

## Work with tabs

- Use the **+** button or ++ctrl+t++ to open a tab.
- Click a tab to select it.
- Use its close control or ++ctrl+w++ to close it.
- Use ++ctrl+tab++ to move forward and ++ctrl+shift+tab++ to move backward.

Hermes keeps one Servo WebView per tab. A background WebView is throttled when it is no longer visible, then resumed when you return to it.

!!! note
    Hermes always keeps at least one tab open. Closing the final remaining tab has no effect.

