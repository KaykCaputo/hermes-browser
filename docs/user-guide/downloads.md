---
title: Downloads
description: Save links, images and pages, find your downloads, and what Mercury does with files the engine cannot show.
---

# Downloads

## Saving files

- **Save page as…** (Cmd+S on macOS, Ctrl+S elsewhere) saves the page's HTML as the site
  serves it. Images, style sheets and scripts are not saved with it.
- Files go to your Downloads folder. Mercury picks the name from the site's answer and never
  replaces a file that is already there: it adds a number instead.
- On macOS, every downloaded file carries the usual "downloaded from the internet" mark, so the
  system asks before opening a program for the first time.

## The downloads list

- The ↓ button appears in the toolbar once something has been downloaded in this session. While
  files are arriving it shows a progress ring, and a dot means something started or finished
  since you last looked.
- Click it, or press Cmd+Shift+J (Ctrl+J elsewhere), to see your recent downloads. From there
  you can:
  - open a file, or show it in Finder;
  - cancel a download, resume an interrupted one, or try again;
  - remove an entry, or clear the whole list.
- Programs, installers, scripts, web pages and shortcut files get no "Open" button, only "Show
  in Finder", so a download is never one click away from running.
- If you quit while files are still downloading, Mercury asks first. Downloads stopped this way
  can be resumed the next time you open Mercury, if the site allows it.

## Files the engine cannot show

Mercury's engine, Servo, does not download files itself. When a link leads to a file it cannot
display (a ZIP archive, a PDF, an installer), Mercury notices, goes back to the page you were
on, and fetches the file again itself:

- **If you clicked the link or typed the address,** the file downloads straight away.
- **If the page started it on its own,** a bar at the top of the page asks: "<site> wants to
  download <file>", with **Download**, **Block** (no more downloads started by that site for the
  rest of the session) and **Not now**. A page in a background tab cannot start a download at
  all.
- **Some downloads cannot be saved:**
  - files sent in answer to a form (a POST), because Mercury cannot send the form again;
  - files made by the page itself (`blob:` and `data:` addresses).

  The downloads list says so.
- **Links that work only once** (some download tokens and expiring links) may fail, because the
  file is fetched a second time. The list then says "The site did not send the file again;
  download it from the page".

!!! note "Files are transferred twice for now"
    The engine has already fetched the whole file by the time Mercury notices it, and that copy
    is thrown away; Mercury then fetches it again. A large download therefore uses twice the
    bandwidth. Fixing this needs a change inside the engine, which is planned.

Files of types the engine *can* show (text, images) open in the tab even when the site marks
them as downloads. To save one, use **Save page as…**.
